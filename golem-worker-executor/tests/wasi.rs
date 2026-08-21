// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_common::model::OwnedAgentId;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::worker::{
    AgentConfigEntryDto, AgentFileSystemNode, AgentFileSystemNodeKind, RevertToOplogIndex,
    RevertWorkerTarget,
};
use golem_common::model::{AgentStatus, IdempotencyKey, OplogIndex, RetryConfig};
use golem_common::schema::SchemaValue;
use golem_common::schema::schema_value::ResultValuePayload;
use golem_test_framework::dsl::{
    TestDsl, count_agent_invocation_pair_since, drain_connection, stderr_events, stdout_events,
};
use golem_test_framework::model::IFSEntry;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
#[cfg(target_os = "linux")]
use golem_worker_executor_test_utils::start_with_agent_storage_quota_on_managed_xfs;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
#[cfg(target_os = "linux")]
use golem_worker_executor_test_utils::{
    start_with_agent_storage_and_object_quota_on_managed_xfs,
    start_with_mutable_agent_storage_quota_on_managed_xfs,
};
use http::{HeaderMap, StatusCode};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tracing::{Instrument, debug, info};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("initial_file_system")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn sorted_config_entries(value: SchemaValue) -> SchemaValue {
    let SchemaValue::List { mut elements } = value else {
        return value;
    };

    elements.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));

    SchemaValue::List { elements }
}

fn schema_string_list(result: SchemaValue) -> Vec<String> {
    let SchemaValue::List { elements } = result else {
        panic!("expected list, got {result:?}")
    };
    elements
        .into_iter()
        .map(|element| match element {
            SchemaValue::String(entry) => entry,
            other => panic!("expected string, got {other:?}"),
        })
        .collect()
}

async fn assert_reconstructed_writable_file(
    executor: &TestWorkerExecutor,
    component: &golem_common::base_model::component::ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
) -> anyhow::Result<()> {
    let result = executor
        .invoke_and_await_agent(
            component,
            agent_id,
            "inspect_writable",
            golem_common::data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        schema_string_list(result),
        vec![
            "p2_read=p3-to-p2".to_string(),
            "p3_read=p3-to-p2".to_string(),
        ]
    );
    Ok(())
}

fn full_replay_config(config: &mut golem_worker_executor::services::golem_config::GolemConfig) {
    config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
    config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
}

#[cfg(target_os = "linux")]
fn managed_xfs_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

#[test]
#[tracing::instrument]
async fn write_stdout(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Logging", "write-stdout-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "write_stdout", data_value!())
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Logging", "write-stderr-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "write_stderr", data_value!())
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Io", "read-stdin-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clocks", "clocks-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "use_std_time_apis", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let SchemaValue::Record { fields } = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 3);

    let SchemaValue::F64(elapsed1) = &fields[0] else {
        panic!("expected f64")
    };
    let SchemaValue::F64(elapsed2) = &fields[1] else {
        panic!("expected f64")
    };
    let SchemaValue::String(odt) = &fields[2] else {
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env(
            "FileSystem",
            vec![("RUST_BACKTRACE".to_string(), "full".to_string())],
        )
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-write-read-delete-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
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
        SchemaValue::Record {
            fields: vec![
                SchemaValue::Option { inner: None },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("hello world".to_string())))
                },
                SchemaValue::Option { inner: None }
            ]
        }
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_read_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_id!("FileReadWrite", "initial-file-read-write-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        SchemaValue::Tuple {
            elements: vec![
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("foo\n".to_string())))
                },
                SchemaValue::Option { inner: None },
                SchemaValue::Option { inner: None },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("baz\n".to_string())))
                },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("hello world".to_string())))
                },
            ]
        }
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_p3_parity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    initial_file_p3_parity_with_backend(last_unique_id, deps, initial_file_system, None).await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[tracing::instrument]
async fn initial_file_p2_p3_parity_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let root = managed_xfs_test_root();
    initial_file_p3_parity_with_backend(last_unique_id, deps, initial_file_system, Some(root)).await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[tracing::instrument]
async fn p2_p3_quota_exhaustion_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let executor =
        start_with_agent_storage_quota_on_managed_xfs(deps, &context, 1024 * 1024, root).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let p2_agent = agent_id!("P3FileSystem", "managed-quota-p2");

    let result = executor
        .invoke_and_await_agent(&component, &p2_agent, "run_p2_quota_surface", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let SchemaValue::List { elements } = result else {
        panic!("expected list, got {result:?}")
    };
    assert_eq!(
        elements,
        vec![
            SchemaValue::String("p2_wrote_before_limit=true".to_string()),
            SchemaValue::String("p2_growth_denied=true".to_string()),
        ]
    );

    let p3_within_quota_agent = agent_id!("P3FileSystem", "managed-quota-p3-within-limit");
    let p3_within_quota = executor
        .invoke_and_await_agent(
            &component,
            &p3_within_quota_agent,
            "run_p3_with_quota",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(p3_within_quota, SchemaValue::Bool(true));

    let p2_exhaustion_agent = agent_id!("P3FileSystem", "managed-quota-p2-exhaustion");
    let p2_exhaustion_worker = executor
        .start_agent(&component.id, p2_exhaustion_agent.clone())
        .await?;
    let p2_exhaustion = schema_string_list(
        executor
            .invoke_and_await_agent(
                &component,
                &p2_exhaustion_agent,
                "exhaust_p2_quota",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected P2 exhaustion result"))?,
    );
    assert_eq!(
        p2_exhaustion.first().map(String::as_str),
        Some("completion=err:quota"),
        "P2 growth must fail specifically because the project quota is exhausted"
    );
    assert_eq!(
        p2_exhaustion.get(1).map(String::as_str),
        Some("prefix-persisted=true")
    );
    let persisted_p2_prefix = schema_string_list(
        executor
            .invoke_and_await_agent(
                &component,
                &p2_exhaustion_agent,
                "inspect_p2_exhaustion",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected persisted P2 failure prefix"))?,
    );
    let p2_size = persisted_p2_prefix
        .first()
        .and_then(|size| size.strip_prefix("size="))
        .and_then(|size| size.parse::<u64>().ok())
        .expect("P2 exhaustion inspection must report a numeric size");
    assert!((4096..=1024 * 1024).contains(&p2_size));
    assert_eq!(
        &persisted_p2_prefix[1..],
        ["prefix-complete=true", "suffix-bytes=0"]
    );
    executor.simulated_crash(&p2_exhaustion_worker).await?;
    let reconstructed_p2_prefix = executor
        .invoke_and_await_agent(
            &component,
            &p2_exhaustion_agent,
            "inspect_p2_exhaustion",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected reconstructed P2 failure prefix"))?;
    assert_eq!(
        schema_string_list(reconstructed_p2_prefix),
        persisted_p2_prefix
    );

    let p3_exhaustion_agent = agent_id!("P3FileSystem", "managed-quota-p3-exhaustion");
    let p3_exhaustion_worker = executor
        .start_agent(&component.id, p3_exhaustion_agent.clone())
        .await?;
    let p3_exhaustion = executor
        .invoke_and_await_agent(
            &component,
            &p3_exhaustion_agent,
            "exhaust_p3_quota",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected P3 exhaustion result"))?;
    let p3_exhaustion = schema_string_list(p3_exhaustion);
    assert_eq!(
        p3_exhaustion.first().map(String::as_str),
        Some("completion=err:quota"),
        "P3 growth must fail specifically because the project quota is exhausted"
    );
    let unwritten_bytes = p3_exhaustion
        .get(2)
        .and_then(|value| value.strip_prefix("unwritten-bytes="))
        .ok_or_else(|| anyhow!("P3 stream input result was not reported: {p3_exhaustion:?}"))?
        .parse::<usize>()?;
    assert_eq!(
        p3_exhaustion.get(1).map(String::as_str),
        Some("prefix-persisted=true"),
        "P3 stream must acknowledge data persisted before quota denial"
    );
    assert!(
        unwritten_bytes > 0,
        "P3 quota failure must return the input suffix that was not persisted"
    );
    let persisted_prefix = executor
        .invoke_and_await_agent(
            &component,
            &p3_exhaustion_agent,
            "inspect_p3_exhaustion",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected persisted P3 failure prefix"))?;
    let persisted_prefix = schema_string_list(persisted_prefix);
    assert_eq!(
        persisted_prefix.get(1).map(String::as_str),
        Some("prefix-complete=true")
    );
    executor.simulated_crash(&p3_exhaustion_worker).await?;
    let reconstructed_prefix = executor
        .invoke_and_await_agent(
            &component,
            &p3_exhaustion_agent,
            "inspect_p3_exhaustion",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected reconstructed P3 failure prefix"))?;
    assert_eq!(schema_string_list(reconstructed_prefix), persisted_prefix);

    for (agent, method, expected) in [
        (
            agent_id!("P3FileSystem", "managed-quota-p2-matrix"),
            "run_p2_quota_matrix",
            vec![
                "direct=true",
                "positioned-stream=true",
                "append=true",
                "sparse-resize=true",
                "overwrite=true",
                "truncate=true",
                "splice=true",
                "hard-link=true",
                "first-unlink=true",
                "open-unlinked=true",
                "rename-replace=true",
                "grew=true",
                "growth-denied=true",
            ],
        ),
        (
            agent_id!("P3FileSystem", "managed-quota-p3-matrix"),
            "run_p3_quota_matrix",
            vec![
                "positioned-stream=true",
                "append=true",
                "sparse-resize=true",
                "overwrite=true",
                "truncate=true",
                "hard-link=true",
                "first-unlink=true",
                "open-unlinked=true",
                "rename-replace=true",
                "growth-denied=true",
            ],
        ),
    ] {
        let result = executor
            .invoke_and_await_agent(&component, &agent, method, data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value from {method}"))?;
        assert_eq!(schema_string_list(result), expected);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn fill_filesystem_leaving(path: &std::path::Path, reserve_bytes: u64) -> anyhow::Result<()> {
    use std::io::Write;

    let mut filler = std::fs::File::create(path)?;
    let block = vec![0x7f; 1024 * 1024];
    let mut length = 0u64;
    loop {
        match filler.write(&block) {
            Ok(0) => break,
            Ok(written) => length += written as u64,
            Err(error) if error.raw_os_error() == Some(28) => break,
            Err(error) => return Err(error.into()),
        }
    }
    filler.sync_all()?;
    filler.set_len(length.saturating_sub(reserve_bytes))?;
    filler.sync_all()?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn p2_p3_mid_effect_enospc_reconstructs_on_unmanaged_filesystem(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let mount_root = managed_xfs_test_root();
    let runtime_root = mount_root.join("unmanaged-mid-effect-runtime");
    let filler_path = mount_root.join("unmanaged-mid-effect-filler");
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.deterministic_root_dir = Some(runtime_root.clone());
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;

    for (agent_id, exhaust_method, inspect_method) in [
        (
            agent_id!("P3FileSystem", "unmanaged-enospc-p2"),
            "exhaust_p2_quota",
            "inspect_p2_exhaustion",
        ),
        (
            agent_id!("P3FileSystem", "unmanaged-enospc-p3"),
            "exhaust_p3_quota",
            "inspect_p3_exhaustion",
        ),
    ] {
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        fill_filesystem_leaving(&filler_path, 256 * 1024)?;
        let failure = schema_string_list(
            executor
                .invoke_and_await_agent(&component, &agent_id, exhaust_method, data_value!())
                .await?
                .into_return_value()
                .ok_or_else(|| anyhow!("expected {exhaust_method} result"))?,
        );
        assert_eq!(
            failure.get(1).map(String::as_str),
            Some("prefix-persisted=true")
        );
        std::fs::remove_file(&filler_path)?;

        let persisted = schema_string_list(
            executor
                .invoke_and_await_agent(&component, &agent_id, inspect_method, data_value!())
                .await?
                .into_return_value()
                .ok_or_else(|| anyhow!("expected {inspect_method} result"))?,
        );
        assert_eq!(
            persisted.get(1).map(String::as_str),
            Some("prefix-complete=true")
        );
        assert!(
            persisted
                .first()
                .and_then(|entry| entry.strip_prefix("size="))
                .and_then(|size| size.parse::<u64>().ok())
                .is_some_and(|size| (4096..2 * 1024 * 1024 + 4096).contains(&size)),
            "{exhaust_method} did not retain only its completed prefix: {persisted:?}"
        );

        executor.simulated_crash(&worker_id).await?;
        let reconstructed = executor
            .invoke_and_await_agent(&component, &agent_id, inspect_method, data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected reconstructed {inspect_method} result"))?;
        assert_eq!(schema_string_list(reconstructed), persisted);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[tracing::instrument]
async fn p2_p3_object_quota_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_and_object_quota_on_managed_xfs(
        deps,
        &context,
        16 * 1024 * 1024,
        32,
        root,
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let expected = vec![
        "hard-link-same-inode=true",
        "object-denied=true",
        "directory-denied=true",
        "symlink-denied=true",
        "denied-while-open=true",
    ];
    for (agent, method, completion_method) in [
        (
            agent_id!("P3FileSystem", "managed-object-quota-p2"),
            "run_p2_object_quota",
            "complete_p2_object_quota_release",
        ),
        (
            agent_id!("P3FileSystem", "managed-object-quota-p3"),
            "run_p3_object_quota",
            "complete_p3_object_quota_release",
        ),
    ] {
        let result = executor
            .invoke_and_await_agent(&component, &agent, method, data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value from {method}"))?;
        assert_eq!(schema_string_list(result), expected);
        let admitted_after_close = executor
            .invoke_and_await_agent(&component, &agent, completion_method, data_value!())
            .await?
            .into_return_value();
        assert_eq!(admitted_after_close, Some(SchemaValue::Bool(true)));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_downgrade_blocks_guest_until_limit_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let (executor, quota) = start_with_mutable_agent_storage_quota_on_managed_xfs(
        deps,
        &context,
        1024 * 1024,
        root.clone(),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo-copy.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "managed-quota-downgrade");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let initial_result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "confirm_invocation_started",
            data_value!(),
        )
        .await?;
    assert_eq!(
        initial_result.into_return_value(),
        Some(SchemaValue::String("executed".to_string()))
    );
    let runtime_path = root
        .join(context.default_environment_id.to_string())
        .join(component.id.to_string())
        .join(worker_id.agent_name_encoded());
    assert!(runtime_path.exists());

    quota.set_limit(4096).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while runtime_path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("over-limit runtime filesystem was not deleted");

    let before_blocked_invocation = executor.oplog_max_index(&worker_id).await?;
    let blocked_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        )
        .await?;
    let blocked = tokio::time::timeout(
        Duration::from_secs(1),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        ),
    )
    .await;
    assert!(
        !matches!(blocked, Ok(Ok(_))),
        "pending invocation completed while reconstruction exceeded its installed quota"
    );
    let blocked_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&blocked_oplog, before_blocked_invocation),
        (0, 0),
        "over-limit reconstruction must not start the pending guest invocation"
    );

    quota.set_limit(1024 * 1024).await?;
    executor.resume(&worker_id, false).await?;
    let recovered = tokio::time::timeout(
        Duration::from_secs(20),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        ),
    )
    .await
    .expect("pending invocation did not recover after raising the limit")?;
    assert_eq!(
        recovered.into_return_value(),
        Some(SchemaValue::String("executed".to_string()))
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    assert!(runtime_path.exists());
    let recovered_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&recovered_oplog, before_blocked_invocation),
        (1, 1)
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn managed_xfs_resource_billing_survives_idle_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let (executor, billing) = start_with_mutable_agent_storage_quota_on_managed_xfs(
        deps,
        &context,
        1024 * 1024,
        managed_xfs_test_root(),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "managed-xfs-resource-billing");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let mutation = executor
        .invoke_and_await_agent(&component, &agent, "run_writable", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected run_writable return value"))?;
    assert_eq!(
        schema_string_list(mutation),
        vec![
            "p2_write_p3_read=p2-to-p3".to_string(),
            "p3_write_p2_read=p3-to-p2".to_string(),
        ]
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    let after_mutation = billing.flush_durable_storage_byte_seconds();
    let mut after_active_window = after_mutation;
    for _ in 0..32 {
        assert_reconstructed_writable_file(&executor, &component, &agent).await?;
        executor
            .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
            .await?;
        after_active_window = billing.flush_durable_storage_byte_seconds();
        if after_active_window > after_mutation {
            break;
        }
    }
    assert!(
        after_active_window > after_mutation,
        "authoritative managed-XFS allocation remained zero-billed across active windows"
    );

    let mut idle_start = billing.flush_durable_storage_byte_seconds();
    let mut stable_samples = 0;
    while stable_samples < 3 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let current = billing.flush_durable_storage_byte_seconds();
        if current == idle_start {
            stable_samples += 1;
        } else {
            idle_start = current;
            stable_samples = 0;
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let idle_end = billing.flush_durable_storage_byte_seconds();
    assert_eq!(
        idle_end, idle_start,
        "loaded-idle managed-XFS storage continued billing"
    );

    executor.simulated_crash(&worker_id).await?;
    assert_reconstructed_writable_file(&executor, &component, &agent).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let after_replay = billing.flush_durable_storage_byte_seconds();
    assert!(
        after_replay > idle_end,
        "replay and its following invocation produced no managed-XFS storage billing"
    );

    let before_delete = billing.flush_durable_storage_byte_seconds();
    executor.delete_worker(&worker_id).await?;
    let after_delete = billing.flush_durable_storage_byte_seconds();
    assert!(after_delete >= before_delete);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        billing.flush_durable_storage_byte_seconds(),
        after_delete,
        "deleted managed-XFS storage continued billing"
    );
    Ok(())
}

async fn initial_file_p3_parity_with_backend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
    managed_xfs_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = match managed_xfs_root {
        Some(root) => {
            start_with_overrides(
                deps,
                &context,
                TestExecutorOverrides {
                    configure: Some(Arc::new(move |config| {
                        config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                    })),
                    ..TestExecutorOverrides::default()
                },
            )
            .await?
        }
        None => start(deps, &context).await?,
    };

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo-copy.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;

    let agent_id = agent_id!("P3FileSystem", "initial-file-p3-parity-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let abandoned_completion = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "abandon_p3_write_completion",
            data_value!(),
        )
        .await?
        .into_return_value();
    assert_eq!(abandoned_completion, Some(SchemaValue::Bool(true)));

    let expected = vec![
        "ro_flags_p2_write=false".to_string(),
        "ro_flags_p3_write=false".to_string(),
        "ro_hash_parity=true".to_string(),
        "ro_hash_p3_deterministic=true".to_string(),
        "ro_hash_at_parity=true".to_string(),
        "ro_set_times_p2=err:not-permitted".to_string(),
        "ro_set_times_p3=err:not-permitted".to_string(),
        "ro_set_times_at_p2=err:not-directory".to_string(),
        "ro_set_times_at_p3=err:not-directory".to_string(),
        "ro_rename_at_p2=err:not-directory".to_string(),
        "ro_rename_at_p3=err:not-directory".to_string(),
        "ro_symlink_at_p2=err:not-directory".to_string(),
        "ro_symlink_at_p3=err:not-directory".to_string(),
        "ro_unlink_file_at_p2=err:not-directory".to_string(),
        "ro_unlink_file_at_p3=err:not-directory".to_string(),
        "ro_parent_open_write_p2=err:not-permitted".to_string(),
        "ro_parent_open_write_p3=err:not-permitted".to_string(),
        "ro_invalid_flags_p2=err:unsupported".to_string(),
        "ro_invalid_flags_p3=err:unsupported".to_string(),
        "ro_parent_unlink_p2=err:not-permitted".to_string(),
        "ro_parent_unlink_p3=err:not-permitted".to_string(),
        "ro_parent_rename_p2=err:not-permitted".to_string(),
        "ro_parent_rename_p3=err:not-permitted".to_string(),
        "ro_parent_link_p2=err:not-permitted".to_string(),
        "ro_parent_link_p3=err:not-permitted".to_string(),
        "ro_alias_create_p2=ok".to_string(),
        "ro_alias_open_write_p2=err:not-permitted".to_string(),
        "ro_alias_unlink_p2=ok".to_string(),
        "ro_alias_create_p3=ok".to_string(),
        "ro_alias_open_write_p3=err:not-permitted".to_string(),
        "ro_alias_unlink_p3=ok".to_string(),
        "rw_flags_p2_write=true".to_string(),
        "rw_flags_p3_write=true".to_string(),
        "rw_hash_parity=true".to_string(),
        "rw_set_times_p2=ok".to_string(),
        "rw_set_times_p3=ok".to_string(),
        "p2_write_p3_read=p2-to-p3".to_string(),
        "p3_write_p2_read=p3-to-p2".to_string(),
    ];

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(schema_string_list(result), expected);

    // Crash the worker so the next invocation replays the recorded oplog first,
    // verifying the P3 metadata-hash -> durable stat call sequence is replay-stable
    // and that replay reconstructed the mutable initial file without help from
    // the verification invocation.
    executor.simulated_crash(&worker_id).await?;

    let result_after_crash = executor
        .invoke_and_await_agent(&component, &agent_id, "inspect_run", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        schema_string_list(result_after_crash),
        vec![
            "p2_read=p3-to-p2".to_string(),
            "p3_read=p3-to-p2".to_string(),
        ]
    );

    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_mutation_histories_reconstruct_from_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_mutation_histories_reconstruct_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        None,
    )
    .await
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn cross_preview_append_coordination_survives_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "cross-preview-append");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let coordinated = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "run_cross_preview_append",
            data_value!(),
        )
        .await?
        .into_return_value();
    assert_eq!(coordinated, Some(SchemaValue::Bool(true)));

    executor.simulated_crash(&worker_id).await?;
    let reconstructed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "inspect_cross_preview_append",
            data_value!(),
        )
        .await?
        .into_return_value();
    assert_eq!(reconstructed, Some(SchemaValue::Bool(true)));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_mutation_histories_reconstruct_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_mutation_histories_reconstruct_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        Some(managed_xfs_test_root()),
    )
    .await
}

async fn filesystem_mutation_histories_reconstruct_with_backend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
    managed_xfs_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                if let Some(root) = &managed_xfs_root {
                    config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                }
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "filesystem-reconstruction-matrix");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let expected = schema_string_list(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "run_reconstruction_matrix",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected reconstruction matrix"))?,
    );
    assert_eq!(
        expected,
        vec![
            "p2-resize=abcdef",
            "p2-times=946684800:0",
            "p2-append=p2-append",
            "p2-directory=true:removed=true",
            "p2-splice=splice-data",
            "p2-hard=hard-p2:2",
            "p2-hard-link=hard-p2:2",
            "p2-symlink=replay-p2-hard.bin:hard-p2",
            "p2-replacement=new-p2",
            "p2-open-unlinked-absent=true",
            "p3-resize=uvwxyz",
            "p3-times=978307200:0",
            "p3-append=p3-append",
            "p3-directory=true:removed=true",
            "p3-hard=hard-p3:2",
            "p3-hard-link=hard-p3:2",
            "p3-symlink=replay-p3-hard.bin:hard-p3",
            "p3-replacement=new-p3",
            "p3-open-unlinked-absent=true",
        ]
    );

    executor.simulated_crash(&worker_id).await?;
    let reconstructed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "inspect_reconstruction_matrix",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected reconstructed matrix"))?;
    assert_eq!(schema_string_list(reconstructed), expected);
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_reconstruction_stops_at_exact_revert_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_reconstruction_stops_at_exact_revert_target_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        None,
    )
    .await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_reconstruction_stops_at_exact_revert_target_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_reconstruction_stops_at_exact_revert_target_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        Some(managed_xfs_test_root()),
    )
    .await
}

async fn filesystem_reconstruction_stops_at_exact_revert_target_with_backend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
    managed_xfs_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                if let Some(root) = &managed_xfs_root {
                    config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                }
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "filesystem-exact-replay-target");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_replay_target",
            data_value!("first"),
        )
        .await?;
    let first_target = executor.oplog_max_index(&worker_id).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_replay_target",
            data_value!("second"),
        )
        .await?;

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: first_target,
            }),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("replay-target.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected replay target contents"))?;
    assert_eq!(
        schema_string_list(result),
        vec!["p2_read=first", "p3_read=first"]
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_reconstruction_uses_updated_initial_files(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_reconstruction_uses_updated_initial_files_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        None,
    )
    .await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_reconstruction_uses_updated_initial_files_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_reconstruction_uses_updated_initial_files_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        Some(managed_xfs_test_root()),
    )
    .await
}

async fn filesystem_reconstruction_uses_updated_initial_files_with_backend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
    managed_xfs_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                if let Some(root) = &managed_xfs_root {
                    config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                }
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: CanonicalFilePath::from_abs_str("/versioned.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "filesystem-updated-initial-files");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/versioned.txt")
            .await?,
        Bytes::from_static(b"baz\n")
    );

    let updated = executor
        .update_component_with_files(
            &component.id,
            "P3FileSystem",
            &initial_file_system.wasm_name,
            vec![IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: CanonicalFilePath::from_abs_str("/versioned.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .await?;
    executor
        .auto_update_worker(&worker_id, updated.revision, false)
        .await?;

    let updated_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("versioned.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected updated initial file"))?;
    assert_eq!(
        schema_string_list(updated_result),
        vec!["p2_read=foo\n", "p3_read=foo\n"]
    );

    executor.simulated_crash(&worker_id).await?;
    let reconstructed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("versioned.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected reconstructed updated initial file"))?;
    assert_eq!(
        schema_string_list(reconstructed),
        vec!["p2_read=foo\n", "p3_read=foo\n"]
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_full_replay_survives_lifecycle_transitions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_full_replay_survives_lifecycle_transitions_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        None,
    )
    .await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_full_replay_survives_managed_xfs_lifecycle_transitions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    filesystem_full_replay_survives_lifecycle_transitions_with_backend(
        last_unique_id,
        deps,
        initial_file_system,
        Some(managed_xfs_test_root()),
    )
    .await
}

async fn filesystem_full_replay_survives_lifecycle_transitions_with_backend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
    managed_xfs_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::shardmanager::ShardId;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
    };
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let first_root = tempfile::tempdir()?;
    let second_root = tempfile::tempdir()?;
    let managed = managed_xfs_root.is_some();
    let (first_backend_root, second_backend_root) = match managed_xfs_root {
        Some(root) => {
            let first = root.join("lifecycle-first-executor");
            let second = root.join("lifecycle-second-executor");
            std::fs::create_dir_all(&first)?;
            std::fs::create_dir_all(&second)?;
            (first, second)
        }
        None => (
            first_root.path().to_path_buf(),
            second_root.path().to_path_buf(),
        ),
    };
    let root = first_backend_root;
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                if managed {
                    config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                } else {
                    config.filesystem_storage.deterministic_root_dir = Some(root.clone());
                }
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "filesystem-full-replay-lifecycles");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run_writable", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        schema_string_list(result),
        vec![
            "p2_write_p3_read=p2-to-p3".to_string(),
            "p3_write_p2_read=p3-to-p2".to_string(),
        ]
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_eviction_class(&owned_agent_id).await
            != Some(golem_worker_executor::worker::EvictionClass::LoadedIdle)
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("worker did not become loaded-idle after its billing window closed"))?;
    assert!(executor.stop_worker_if_idle(&owned_agent_id).await?);
    assert!(!executor.worker_is_loaded(&owned_agent_id).await);
    assert_reconstructed_writable_file(&executor, &component, &agent_id).await?;

    let shard = ShardId { value: 0 };
    let mut client = executor.client.clone();
    let revoked = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![shard],
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while executor.worker_is_loaded(&owned_agent_id).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow!("worker remained loaded after its shard was revoked"))?;
    assert!(!executor.worker_is_loaded(&owned_agent_id).await);
    let assigned = client
        .assign_shards(AssignShardsRequest {
            shard_ids: vec![shard],
        })
        .await?
        .into_inner();
    assert!(matches!(
        assigned.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    assert_reconstructed_writable_file(&executor, &component, &agent_id).await?;

    drop(client);
    drop(executor);

    let root = second_backend_root;
    let relocated = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                if managed {
                    config.filesystem_storage.managed_xfs_root_dir = Some(root.clone());
                } else {
                    config.filesystem_storage.deterministic_root_dir = Some(root.clone());
                }
                full_replay_config(config);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    assert_reconstructed_writable_file(&relocated, &component, &agent_id).await?;
    relocated.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_listing_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::agent_id;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;

    let agent_id = agent_id!("FileReadWrite", "initial-file-listing-1");
    let worker_id = executor.start_agent(&component.id, agent_id).await?;

    let result = executor.get_file_system_node(&worker_id, "/").await?;

    let mut result = result
        .into_iter()
        .map(|e| AgentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![
            AgentFileSystemNode {
                name: "bar".to_string(),
                last_modified: 0,
                kind: AgentFileSystemNodeKind::Directory,
                permissions: None,
                size: None
            },
            AgentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: 0,
                kind: AgentFileSystemNodeKind::File,
                permissions: Some(AgentFilePermissions::ReadWrite),
                size: Some(4),
            },
            AgentFileSystemNode {
                name: "foo.txt".to_string(),
                last_modified: 0,
                kind: AgentFileSystemNodeKind::File,
                permissions: Some(AgentFilePermissions::ReadOnly),
                size: Some(4)
            },
        ]
    );

    let result = executor.get_file_system_node(&worker_id, "/bar").await?;

    let mut result = result
        .into_iter()
        .map(|e| AgentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![AgentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: AgentFileSystemNodeKind::File,
            permissions: Some(AgentFilePermissions::ReadWrite),
            size: Some(4),
        },]
    );

    let result = executor
        .get_file_system_node(&worker_id, "/baz.txt")
        .await?;

    let mut result = result
        .into_iter()
        .map(|e| AgentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![AgentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: AgentFileSystemNodeKind::File,
            permissions: Some(AgentFilePermissions::ReadWrite),
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
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_id!("FileReadWrite", "initial-file-read-write-3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // run the agent so it can update the files.
    executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run_directories", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let SchemaValue::Record { fields } = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 4);

    assert_eq!(fields[0], SchemaValue::U32(0)); // initial number of entries
    assert_eq!(
        fields[1],
        SchemaValue::List {
            elements: vec![SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test".to_string()),
                    SchemaValue::Bool(true)
                ]
            }]
        }
    ); // contents of /

    // contents of /test
    let SchemaValue::List { elements: list } = &fields[2] else {
        panic!("expected list")
    };
    assert_eq!(
        *list,
        vec![
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/dir1".to_string()),
                    SchemaValue::Bool(true)
                ]
            },
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/dir2".to_string()),
                    SchemaValue::Bool(true)
                ]
            },
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/hello.txt".to_string()),
                    SchemaValue::Bool(false)
                ]
            },
        ]
    );
    assert_eq!(fields[3], SchemaValue::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked

    Ok(())
}

#[test]
#[tracing::instrument]
async fn directories_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run_directories", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // NOTE: if the directory listing would not be stable, replay would fail with divergence error

    let metadata = executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(5))
        .await?;

    assert_eq!(metadata.status, AgentStatus::Idle);

    let SchemaValue::Record { fields } = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 4);

    assert_eq!(fields[0], SchemaValue::U32(0)); // initial number of entries
    assert_eq!(
        fields[1],
        SchemaValue::List {
            elements: vec![SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test".to_string()),
                    SchemaValue::Bool(true)
                ]
            }]
        }
    ); // contents of /

    // contents of /test
    let SchemaValue::List { elements: list } = &fields[2] else {
        panic!("expected list")
    };
    assert_eq!(
        *list,
        vec![
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/dir1".to_string()),
                    SchemaValue::Bool(true)
                ]
            },
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/dir2".to_string()),
                    SchemaValue::Bool(true)
                ]
            },
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("/test/hello.txt".to_string()),
                    SchemaValue::Bool(false)
                ]
            },
        ]
    );
    assert_eq!(fields[3], SchemaValue::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_write_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
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
            &component,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("hello world".to_string())))
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_update_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "IfsUpdate",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .store()
        .await?;

    let agent_id = agent_id!("IfsUpdate", "ifs-update-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_before_update = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_before_update, "foo\n");
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "IfsUpdate",
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/bar.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                }],
            )
            .await?;

        executor
            .auto_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_update = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_update, "foo\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_crash, "foo\n");
    }

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_reload, "bar\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_crash, "bar\n");
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "IfsUpdate",
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                }],
            )
            .await?;

        executor
            .manual_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_manual_update = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_manual_update, "restored");
    }

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_reload, "baz\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component, &agent_id, "get_file_content", data_value!())
            .await?
            .into_typed::<String>()?;

        assert_eq!(content_after_crash, "baz\n");
    }

    executor.delete_worker(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_update_in_the_middle_of_exported_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "IfsUpdateInsideExportedFunction",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .with_env(
            "IfsUpdateInsideExportedFunction",
            vec![
                ("PORT".to_string(), host_http_port.to_string()),
                ("RUST_BACKTRACE".to_string(), "full".to_string()),
            ],
        )
        .store()
        .await?;

    let agent_id = agent_id!("IfsUpdateInsideExportedFunction", "ifs-update-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let idempotency_key = IdempotencyKey::fresh();

    executor
        .invoke_agent_with_key(
            &component,
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
                "IfsUpdateInsideExportedFunction",
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/bar.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
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
                &component,
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
            SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::String("foo\n".to_string()),
                    SchemaValue::String("bar\n".to_string())
                ]
            }
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Environment", "environment-service-1");
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    // The same environment must be visible through the P3-native
    // `wasi:cli/environment@0.3` import as through the P2/std path
    let result_p3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_environment_p3", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result_p3, result);

    let worker_name = agent_id.to_string();
    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::List {
                elements: vec![
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("TEST_ENV".to_string()),
                            SchemaValue::String("test-value".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("GOLEM_AGENT_ID".to_string()),
                            SchemaValue::String(worker_name.clone())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("GOLEM_WORKER_NAME".to_string()),
                            SchemaValue::String(worker_name)
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("GOLEM_COMPONENT_ID".to_string()),
                            SchemaValue::String(component.id.to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("GOLEM_COMPONENT_REVISION".to_string()),
                            SchemaValue::String("0".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("GOLEM_AGENT_TYPE".to_string()),
                            SchemaValue::String("Environment".to_string())
                        ]
                    }
                ]
            }))
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_response_persisted_between_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "send_request", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(rx);
    let executor = start(deps, &context).await?;
    let _rx = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "process_response", data_value!())
        .await?;

    http_server.abort();

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_interrupting_response_stream(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_agent_with_key(
                    &component_clone,
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
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(5))
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "slow_body_stream",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result.into_typed::<u64>()?, 100u64 * 1024u64);

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
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient3");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_agent_with_key(
                    &component_clone,
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
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(5))
        .await?;
    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "slow_body_stream",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result.into_typed::<u64>()?, 100u64 * 1024u64);

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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(10u64))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(0u64))
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(1u64))
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?
        .into_typed::<bool>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 1);
    assert!(result);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(12u64))
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?
        .into_typed::<bool>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 12);
    assert!(result);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn p3_sleep_suspends_and_resumes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "p3-sleep-suspends-and-resumes");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let start = Instant::now();
    let mut fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "sleep_p3",
                    data_value!(25u64),
                )
                .await
        }
        .in_current_span(),
    );

    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow!("sleep_p3 returned before suspending: {:?}", invoke_result));
        }
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(15)) => {
            status?;
        }
    }

    fiber.await??;
    let duration = start.elapsed();
    assert!(duration.as_secs() >= 25);

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(0u64))
        .await?;
    assert!(start.elapsed().as_secs() < 2);

    Ok(())
}

/// Interrupting a worker while the guest is parked in a live P3 monotonic-clock wait
/// (`wasi:clocks` `wait-for`) must deliver the interrupt promptly instead of waiting for the
/// sleep to elapse: the park races the worker's interrupt signal, the durable call is abandoned
/// (its `Start` stays incomplete for replay) and the event loop unwinds cooperatively with the
/// interrupt. After resume, the retained invocation replays, re-enters the wait and completes.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn interrupt_while_parked_in_p3_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    // Keep the parked wait from suspending during the test, so the interrupt must be delivered
    // to the *parked* host future rather than to an already-suspended worker.
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "interrupt-while-parked-in-p3-sleep");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "sleep_p3",
                    data_value!(15u64),
                )
                .await
        }
        .in_current_span(),
    );

    // Give the guest time to enter the P3 clock wait, then interrupt.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let interrupted_at = Instant::now();
    executor.interrupt(&worker_id).await?;

    let result = fiber.await?;
    // If the interrupt were not delivered to the parked wait, the sleep would run to completion
    // and the invocation would succeed instead.
    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(
        err_msg.contains("Interrupted via the Golem API"),
        "Expected interruption error, got: {err_msg}"
    );
    assert!(
        interrupted_at.elapsed() < Duration::from_secs(10),
        "interrupting a parked P3 sleep must unwind promptly"
    );

    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // Resuming replays the worker; the retained invocation re-enters the wait and completes
    // once the originally recorded deadline elapses.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(60))
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(0u64))
        .await?;
    assert!(start.elapsed().as_secs() < 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    Ok(())
}

/// Interrupting a worker while the guest is parked in a live P3 TCP `receive` wait (connected,
/// but the peer never sends any bytes) must deliver the interrupt promptly: the parked durable
/// receive task races the worker's interrupt signal, abandons its open chunk child and parent
/// durable calls (both `Start`s stay incomplete for replay) and unwinds the event loop
/// cooperatively, closing the socket. After resume, the retained invocation replays: the guest
/// reconnects live and completes against the now-responding server.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn interrupt_while_parked_in_p3_tcp_receive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (connected_tx, mut connected_rx) = tokio::sync::mpsc::unbounded_channel();
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::unbounded_channel();

    let tcp_server = spawn(
        async move {
            let mut first = true;
            while let Ok((mut stream, _)) = listener.accept().await {
                let is_first = first;
                first = false;
                let connected_tx = connected_tx.clone();
                let closed_tx = closed_tx.clone();
                spawn(async move {
                    if is_first {
                        // Never send anything; report when the peer closes the connection.
                        let _ = connected_tx.send(());
                        let mut byte = [0u8; 1];
                        let closed = matches!(stream.read(&mut byte).await, Ok(0));
                        let _ = closed_tx.send(closed);
                    } else {
                        let _ = stream.write_all(b"hello").await;
                        let _ = stream.shutdown().await;
                    }
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Networking", "interrupt-while-parked-in-p3-tcp-receive");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "tcp_collect_p3",
                    data_value!(port),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait until the guest has connected, then give it a moment to enter the parked receive
    // wait before interrupting.
    tokio::time::timeout(Duration::from_secs(30), connected_rx.recv())
        .await?
        .ok_or_else(|| anyhow!("tcp server stopped before the guest connected"))?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await?;

    let result = fiber.await?;
    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(
        err_msg.contains("Interrupted via the Golem API"),
        "Expected interruption error, got: {err_msg}"
    );

    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    assert!(
        tokio::time::timeout(Duration::from_secs(10), closed_rx.recv())
            .await?
            .unwrap_or(false),
        "interrupting the worker must close the in-flight TCP connection"
    );

    // Resuming replays the worker; the retained invocation reconnects live and completes
    // against the now-responding server.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "tcp_collect_p3", data_value!(port))
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(result2, Ok("hello".to_string()));

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    tcp_server.abort();

    Ok(())
}

async fn simulated_slow_request_server(delay: Duration) -> (u16, JoinHandle<()>) {
    let (port, server, _) = counting_slow_request_server(delay).await;
    (port, server)
}

/// Like [`simulated_slow_request_server`], but also returns a counter of how many requests the
/// server has received.
async fn counting_slow_request_server(delay: Duration) -> (u16, JoinHandle<()>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_clone = request_count.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/simulated-slow-request",
                get(move || async move {
                    request_count_clone.fetch_add(1, Ordering::AcqRel);
                    tokio::time::sleep(delay).await;
                    "slow response".to_string()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    (host_http_port, http_server, request_count)
}

/// Creates an HTTP server with a streaming endpoint that sends many small chunks
/// with delays between them, mimicking OpenAI-style SSE streaming.
/// Each chunk is a small piece of text sent with a configurable delay.
async fn streaming_chunk_server(
    chunk_count: usize,
    chunk_delay: Duration,
) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new()
                .route(
                    "/streaming-chunks",
                    get(move || async move {
                        let stream =
                            stream::iter(0..chunk_count)
                                .throttle(chunk_delay)
                                .map(move |i| {
                                    Ok::<Bytes, BoxError>(Bytes::from(format!("chunk-{i}\n")))
                                });

                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "text/plain")
                            .header("Transfer-Encoding", "chunked")
                            .body(axum::body::Body::from_stream(stream))
                            .unwrap()
                    }),
                )
                .route(
                    "/simulated-slow-request",
                    get(move || async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(10)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-4");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(2u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 2);
    assert!(duration.as_secs() < 10);
    assert_eq!(result, "Timeout");
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(5)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-5");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(30u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 5);
    assert!(duration.as_secs() < 30);
    assert_eq!(result, "slow response");
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(30)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-6");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(15u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 15);
    assert!(duration.as_secs() < 30);
    assert_eq!(result, "Timeout");

    Ok(())
}

/// Mixed-ABI suspend regression: one guest task awaits a slow P3 `wasi:http` send while another
/// blocks in a P2 sleep (`thread::sleep` → `wasi:io/poll`) longer than the suspend threshold.
/// The worker must not be suspended while the P3 request is in flight — a premature suspend
/// would drop the pending host call and re-execute the HTTP request on resume, so the server
/// receiving the request exactly once proves the P3 completion was delivered.
#[test]
#[tracing::instrument]
async fn p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server, request_count) = counting_slow_request_server(Duration::from_secs(5)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-p2-sleep-during-request");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "p2_sleep_during_request",
            data_value!(15u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert_eq!(result, "slow response, slept");
    assert_eq!(
        request_count.load(Ordering::Acquire),
        1,
        "the P3 HTTP request must be sent exactly once; a second request means the worker was \
         suspended while the request was in flight and re-executed it on resume"
    );
    assert!(duration.as_secs() >= 15);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_and_awaiting_parallel_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(2)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-7");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(20u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    server.abort();

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?
        .into_typed::<bool>()?;

    assert!(duration.as_secs() >= 10);
    assert!(duration.as_secs() < 20);
    assert_eq!(
        result,
        "Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n"
    );
    assert!(healthcheck_result);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn jump_with_in_flight_durable_call_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 1,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(2)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "jump-during-request");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "jump_during_request", data_value!())
        .await;

    drop(executor);
    server.abort();

    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("durable host calls are still in flight"),
        "Unexpected error: {err}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_below_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-8");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 5u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?
        .into_typed::<bool>()?;

    assert!(duration.as_secs() >= 10);
    assert_eq!(
        result,
        "Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n"
    );
    assert!(healthcheck_result);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_above_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-9");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(12u64, 2u64),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?
        .into_typed::<bool>()?;

    assert!(duration.as_secs() >= 14);
    assert_eq!(result, "Ok(\"slow response\")\nOk(\"slow response\")\n");
    assert!(healthcheck_result);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn resuming_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
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

    drop(executor);
    fiber.await??;

    info!("Restarting worker...");

    let executor = start(deps, &context).await?;

    info!("Worker restarted");

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(10u64))
        .await?;
    let duration = start.elapsed();

    assert!(duration.as_secs() < 20);
    assert!(duration.as_secs() >= 10);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn p3_resuming_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "p3-resuming-sleep");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_agent(&component, &agent_id, "sleep_p3", data_value!(25u64))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(15))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    info!("Restarting worker...");
    let executor = start(deps, &context).await?;
    info!("Worker restarted");

    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(15))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(0u64))
        .await?;
    assert!(start.elapsed().as_secs() < 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("FailingCounter", "failing-worker-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
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
            &component,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("hello world".to_string())))
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_write_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file_direct",
            data_value!("testfile.txt", "hello world"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(
            &component,
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
            &component,
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-4");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/"))
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-5");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_link",
            data_value!("/testfile.txt", "/link.txt"),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "read_file", data_value!("/link.txt"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("hello world".to_string())))
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_link_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-6");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_link",
            data_value!("/test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_dir_1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times_dir_2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component,
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-7");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test/a"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "remove_directory",
            data_value!("/test/a"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-8");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file_direct",
            data_value!("test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_sym_link",
            data_value!("../test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_dir_1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times_dir_2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component,
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-9");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "rename_file",
            data_value!("/test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_srcdir_1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_destdir_1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component,
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
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_destdir_2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component,
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-10");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "remove_file",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_info", data_value!("/test"))
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(
            &component,
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
            &component,
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file_direct",
            data_value!("testfile.txt", "hello world"),
        )
        .await?;

    let hash1 = executor
        .invoke_and_await_agent(&component, &agent_id, "hash", data_value!("testfile.txt"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let hash2 = executor
        .invoke_and_await_agent(&component, &agent_id, "hash", data_value!("testfile.txt"))
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Networking", "ip-address-resolve-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // If the recovery succeeds, that means that the replayed IP address resolution produced the same result as expected

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the same order) but we can expect
    // that it could resolve golem.cloud to at least one address.
    let SchemaValue::List { elements: entries1 } = &result1 else {
        panic!("expected list, got {:?}", result1)
    };
    let SchemaValue::List { elements: entries2 } = &result2 else {
        panic!("expected list, got {:?}", result2)
    };
    assert!(!entries1.is_empty());
    assert!(!entries2.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn p3_ip_address_resolve(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Networking", "p3-ip-address-resolve-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("golem.cloud"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // If the recovery succeeds, the replayed P3 DNS resolution produced the same recorded result

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("golem.cloud"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the
    // same order) but we can expect that it could resolve golem.cloud to at least one address.
    let SchemaValue::Result(ResultValuePayload::Ok {
        value: Some(entries1),
    }) = &result1
    else {
        panic!("expected successful resolution, got {:?}", result1)
    };
    let SchemaValue::Result(ResultValuePayload::Ok {
        value: Some(entries2),
    }) = &result2
    else {
        panic!("expected successful resolution, got {:?}", result2)
    };
    let SchemaValue::List { elements: entries1 } = entries1.as_ref() else {
        panic!("expected list, got {:?}", entries1)
    };
    let SchemaValue::List { elements: entries2 } = entries2.as_ref() else {
        panic!("expected list, got {:?}", entries2)
    };
    assert!(!entries1.is_empty());
    assert!(!entries2.is_empty());

    Ok(())
}

/// A permanently failing P3 DNS lookup (`NameUnresolvable`) must surface to the guest as an error
/// value without routing through the worker-level retry machinery: the invocation succeeds, the
/// guest observes the error, and the oplog contains no `Error` (retry) entries. The transient
/// side of the classification (`TemporaryResolverFailure` / `Other` raising a retry trap) cannot
/// be induced end-to-end because the underlying resolver maps every lookup failure to
/// `NameUnresolvable`; it is covered by the classification unit tests in
/// `durable_host::p3::sockets`.
#[test]
#[tracing::instrument]
async fn p3_ip_address_resolve_permanent_failure_is_guest_visible_without_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;
    use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Networking", "p3-ip-address-resolve-failure-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("this-name-does-not-exist.golem-test.invalid"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let SchemaValue::Result(ResultValuePayload::Err { value: Some(error) }) = &result else {
        panic!("expected guest-visible resolution error, got {:?}", result)
    };
    let SchemaValue::String(error) = error.as_ref() else {
        panic!("expected error message string, got {:?}", error)
    };
    assert!(
        error.contains("NameUnresolvable"),
        "expected NameUnresolvable, got {error}"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let retry_errors = oplog
        .iter()
        .filter(|e| matches!(e.entry, PublicOplogEntry::Error(_)))
        .count();
    assert_eq!(
        retry_errors, 0,
        "a permanent DNS failure must not trigger worker-level retries"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn wasi_config_initial_worker_config(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WasiConfig", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("v1".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k2".to_string()],
                    value: serde_json::Value::String("v2".to_string()).into(),
                },
            ],
        )
        .await?;

    {
        // get existing key

        let result = executor
            .invoke_and_await_agent(&component, &agent_id, "get", data_value!("k1"))
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("v1".to_string())))
            }
        )
    }

    {
        // get non-existent key

        let result = executor
            .invoke_and_await_agent(&component, &agent_id, "get", data_value!("k3"))
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(result, SchemaValue::Option { inner: None })
    }

    {
        // get all keys

        let result = sorted_config_entries(
            executor
                .invoke_and_await_agent(&component, &agent_id, "get_all", data_value!())
                .await?
                .into_return_value()
                .ok_or_else(|| anyhow!("expected return value"))?,
        );

        assert_eq!(
            result,
            SchemaValue::List {
                elements: vec![
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k1".to_string()),
                            SchemaValue::String("v1".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k2".to_string()),
                            SchemaValue::String("v2".to_string())
                        ]
                    }
                ]
            }
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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_agent_config(
            "WasiConfig",
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("v0".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k3".to_string()],
                    value: serde_json::Value::String("v3".to_string()).into(),
                },
            ],
        )
        .store()
        .await?;

    let agent_id = agent_id!("WasiConfig", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("v1".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k2".to_string()],
                    value: serde_json::Value::String("v2".to_string()).into(),
                },
            ],
        )
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(&component, &agent_id, "get_all", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            SchemaValue::List {
                elements: vec![
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k1".to_string()),
                            SchemaValue::String("v1".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k2".to_string()),
                            SchemaValue::String("v2".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k3".to_string()),
                            SchemaValue::String("v3".to_string())
                        ]
                    },
                ]
            }
        )
    }

    let updated_component = executor
        .update_component_with(
            &component.id,
            component.revision,
            None,
            Some(BTreeMap::from([(
                golem_common::model::agent::AgentTypeName("WasiConfig".to_string()),
                golem_common::model::component::AgentTypeProvisionConfigUpdate {
                    config: Some(vec![
                        AgentConfigEntryDto {
                            path: vec!["k1".to_string()],
                            value: serde_json::Value::String("v2".to_string()).into(),
                        },
                        AgentConfigEntryDto {
                            path: vec!["k3".to_string()],
                            value: serde_json::Value::String("v4".to_string()).into(),
                        },
                        AgentConfigEntryDto {
                            path: vec!["k4".to_string()],
                            value: serde_json::Value::String("v4".to_string()).into(),
                        },
                    ]),
                    ..Default::default()
                },
            )])),
            Vec::new(),
        )
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    {
        let result = sorted_config_entries(
            executor
                .invoke_and_await_agent(&updated_component, &agent_id, "get_all", data_value!())
                .await?
                .into_return_value()
                .ok_or_else(|| anyhow!("expected return value"))?,
        );

        assert_eq!(
            result,
            SchemaValue::List {
                elements: vec![
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k1".to_string()),
                            SchemaValue::String("v1".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k2".to_string()),
                            SchemaValue::String("v2".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k3".to_string()),
                            SchemaValue::String("v4".to_string())
                        ]
                    },
                    SchemaValue::Tuple {
                        elements: vec![
                            SchemaValue::String("k4".to_string()),
                            SchemaValue::String("v4".to_string())
                        ]
                    },
                ]
            }
        )
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

/// Reproducer for oplog mismatch bug: "expected io::poll::poll, got io::poll::ready"
///
/// This test exercises the scenario where a worker does HTTP requests with sleeps
/// between them (triggering suspend due to exceeding the suspend threshold), then
/// the executor is restarted, and the same function is re-invoked on the same worker.
/// This forces a full oplog replay of the HTTP-heavy invocation. If subscribe() calls
/// return different Resource<Pollable> handle IDs during replay, wstd's reactor may
/// iterate its internal HashMap differently, causing ready()/poll() calls to be made
/// in a different order than the oplog expects.
#[test]
#[tracing::instrument]
async fn oplog_replay_after_http_requests_with_suspend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Use a fast response server so the test doesn't take too long
    let (port, server) = simulated_slow_request_server(Duration::from_millis(100)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-oplog-replay-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: do 3 HTTP requests with 1-second sleeps between them.
    // The sleeps are below the suspend threshold so this completes normally,
    // building up a substantial oplog with subscribe/ready/poll entries.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 3u64),
        )
        .await?
        .into_typed::<String>()?;

    assert_eq!(
        result,
        "Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n"
    );

    info!("First invocation completed, dropping executor to force oplog replay on restart");

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor but keep the HTTP server running on the same port
    // so the second invocation can actually connect
    drop(executor);

    // Restart the executor - this simulates the worker being reactivated
    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker (same component object from before restart):
    // this triggers full oplog replay of the first invocation before switching to live
    // mode for the second. If subscribe() returns different handle IDs during replay,
    // the wstd reactor may diverge and call ready() where poll() was expected.
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 2u64),
        )
        .await?
        .into_typed::<String>()?;

    assert_eq!(result2, "Ok(\"slow response\")\nOk(\"slow response\")\n");

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}

/// Reproducer for oplog mismatch bug with CONCURRENT async tasks.
///
/// Uses sleep_during_parallel_requests which races 3 concurrent HTTP request
/// loops against a timeout. This creates complex interleaving of subscribe/ready/poll
/// calls that may produce different Resource<Pollable> handle IDs during replay.
#[test]
#[tracing::instrument]
async fn oplog_replay_after_parallel_http_requests(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Fast response server — each request completes in 100ms
    let (port, server) = simulated_slow_request_server(Duration::from_millis(100)).await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "clock-oplog-parallel-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent HTTP request loops (5 requests each) raced with
    // a 30-second timeout. This creates many subscribe/ready/poll calls with complex
    // interleaving across concurrent async tasks in wstd's reactor.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(30u64),
        )
        .await?
        .into_typed::<String>()?;

    // All 3 concurrent loops should complete (each with 5 "slow response" results)
    let result_str = result.clone();
    let line_count = result_str.lines().count();
    assert!(
        line_count >= 5,
        "Expected at least 5 lines in result, got {}",
        line_count
    );

    info!(
        "First invocation completed with {} result lines, dropping executor",
        line_count
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Keep server running, drop executor
    drop(executor);

    // Restart the executor
    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay of the
    // parallel HTTP invocation. If subscribe() returns different handle IDs during
    // replay, wstd's reactor HashMap iteration may diverge.
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(30u64),
        )
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    let line_count2 = result2_str.lines().count();
    assert!(
        line_count2 >= 5,
        "Expected at least 5 lines in result, got {}",
        line_count2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}

/// Tests that two agents sharing a very limited HTTP connection pool
/// (max 1 total connection) can both complete their requests, even though
/// one agent occupies the pool with a slow streaming body for several seconds.
#[test]
#[tracing::instrument]
async fn http_connection_pool_contention_between_agents(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;
    use golem_worker_executor::services::golem_config::{
        HttpClientConfig, HttpClientEnabledConfig,
    };
    use golem_worker_executor_test_utils::start_with_http_client_config;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_http_client_config(
        deps,
        &context,
        HttpClientConfig::Enabled(HttpClientEnabledConfig {
            max_idle_per_host: 1,
            max_connections_per_host: 1,
            max_total_connections: 1,
            ..Default::default()
        }),
    )
    .await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    // Track when the slow stream starts being consumed
    let (slow_started_tx, mut slow_started_rx) = tokio::sync::mpsc::channel::<()>(1);
    let slow_started_tx = Arc::new(slow_started_tx);

    let http_server = spawn({
        let slow_started_tx = slow_started_tx.clone();
        async move {
            let request_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let route = Router::new().route(
                "/big-byte-array",
                get(move || {
                    let slow_started_tx = slow_started_tx.clone();
                    let request_count = request_count.clone();
                    async move {
                        let n = request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 0 {
                            // First request: slow streaming body (~10s)
                            let _ = slow_started_tx.send(()).await;
                            let stream = stream::iter(0..100)
                                .throttle(Duration::from_millis(100))
                                .map(|_| Ok::<Bytes, BoxError>(Bytes::from(vec![0u8; 1024])));
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "application/octet-stream")
                                .body(axum::body::Body::from_stream(stream))
                                .unwrap()
                        } else {
                            // Subsequent requests: fast response
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "application/octet-stream")
                                .body(axum::body::Body::from(vec![0u8; 2048]))
                                .unwrap()
                        }
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span()
    });

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    let agent_id_slow = {
        use golem_common::phantom_agent_id;
        phantom_agent_id!("StreamingClient", uuid::Uuid::now_v7())
    };
    let agent_id_fast = {
        use golem_common::phantom_agent_id;
        phantom_agent_id!("StreamingClient", uuid::Uuid::now_v7())
    };

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id_slow = executor
        .start_agent_with(
            &component.id,
            agent_id_slow.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let worker_id_fast = executor
        .start_agent_with(&component.id, agent_id_fast.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id_slow).await?;
    executor.log_output(&worker_id_fast).await?;

    // Start the slow agent's request (don't await it yet)
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_slow_clone = agent_id_slow.clone();
    let slow_handle = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_slow_clone,
                    "slow_body_stream",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait for the slow stream to actually start being consumed
    slow_started_rx
        .recv()
        .await
        .expect("slow stream should have started");

    // Now invoke the fast agent — it must wait for the connection pool
    let fast_start = Instant::now();
    let fast_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_fast,
            "slow_body_stream",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    let fast_elapsed = fast_start.elapsed();

    info!("Fast agent completed in {fast_elapsed:?}");

    // The slow agent should also have completed
    let slow_result = slow_handle.await??.into_typed::<u64>()?;

    // The fast invoke should have been blocked by the pool for most of the slow stream's duration
    assert!(
        fast_elapsed >= Duration::from_secs(5),
        "Expected fast agent to be blocked by pool contention, but it completed in {fast_elapsed:?}"
    );

    // slow: 100 chunks * 1024 bytes = 102400
    assert_eq!(slow_result, 100 * 1024);
    // fast: 2048 bytes
    assert_eq!(fast_result, 2048);

    executor.check_oplog_is_queryable(&worker_id_slow).await?;
    executor.check_oplog_is_queryable(&worker_id_fast).await?;

    http_server.abort();
    drop(executor);
    Ok(())
}

/// Same as http_connection_pool_contention_between_agents, but after both
/// invocations complete the executor is dropped and restarted, then both
/// agents are invoked again to verify they recover from oplog replay correctly.
#[test]
#[tracing::instrument]
async fn http_connection_pool_contention_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;
    use golem_worker_executor::services::golem_config::{
        HttpClientConfig, HttpClientEnabledConfig,
    };
    use golem_worker_executor_test_utils::start_with_http_client_config;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_http_client_config(
        deps,
        &context,
        HttpClientConfig::Enabled(HttpClientEnabledConfig {
            max_idle_per_host: 1,
            max_connections_per_host: 1,
            max_total_connections: 1,
            ..Default::default()
        }),
    )
    .await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    // Track when the slow stream starts being consumed
    let (slow_started_tx, mut slow_started_rx) = tokio::sync::mpsc::channel::<()>(1);
    let slow_started_tx = Arc::new(slow_started_tx);

    let http_server = spawn({
        let slow_started_tx = slow_started_tx.clone();
        async move {
            let request_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let route = Router::new().route(
                "/big-byte-array",
                get(move || {
                    let slow_started_tx = slow_started_tx.clone();
                    let request_count = request_count.clone();
                    async move {
                        let n = request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 0 {
                            // First request: slow streaming body (~10s)
                            let _ = slow_started_tx.send(()).await;
                            let stream = stream::iter(0..100)
                                .throttle(Duration::from_millis(100))
                                .map(|_| Ok::<Bytes, BoxError>(Bytes::from(vec![0u8; 1024])));
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "application/octet-stream")
                                .body(axum::body::Body::from_stream(stream))
                                .unwrap()
                        } else {
                            // Subsequent requests: fast response
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "application/octet-stream")
                                .body(axum::body::Body::from(vec![0u8; 2048]))
                                .unwrap()
                        }
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span()
    });

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    let agent_id_slow = {
        use golem_common::phantom_agent_id;
        phantom_agent_id!("StreamingClient", uuid::Uuid::now_v7())
    };
    let agent_id_fast = {
        use golem_common::phantom_agent_id;
        phantom_agent_id!("StreamingClient", uuid::Uuid::now_v7())
    };

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id_slow = executor
        .start_agent_with(
            &component.id,
            agent_id_slow.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let worker_id_fast = executor
        .start_agent_with(&component.id, agent_id_fast.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id_slow).await?;
    executor.log_output(&worker_id_fast).await?;

    // Start the slow agent's request (don't await it yet)
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_slow_clone = agent_id_slow.clone();
    let slow_handle = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_slow_clone,
                    "slow_body_stream",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait for the slow stream to actually start being consumed
    slow_started_rx
        .recv()
        .await
        .expect("slow stream should have started");

    // Now invoke the fast agent with a short timeout — the pool is contended so
    // the HTTP request cannot even start within 2s, causing the timeout to fire.
    let fast_start = Instant::now();
    let fast_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_fast,
            "slow_body_stream_with_timeout",
            data_value!(2000u64), // 2 second timeout
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let fast_elapsed = fast_start.elapsed();

    info!("Fast agent completed in {fast_elapsed:?} with result: {fast_result:?}");

    // The fast agent should have timed out and returned None
    assert_eq!(fast_result, SchemaValue::Option { inner: None });

    // The slow agent should also have completed
    let slow_result = slow_handle.await??.into_typed::<u64>()?;

    // slow: 100 chunks * 1024 bytes = 102400
    assert_eq!(slow_result, 100 * 1024);

    // Drop executor and restart to force oplog replay on both agents
    info!("Dropping executor to force restart...");
    drop(executor);

    info!("Restarting executor...");
    let executor = start_with_http_client_config(
        deps,
        &context,
        HttpClientConfig::Enabled(HttpClientEnabledConfig {
            max_idle_per_host: 1,
            max_connections_per_host: 1,
            max_total_connections: 1,
            ..Default::default()
        }),
    )
    .await?;
    info!("Executor restarted");

    // After restart, invoke both agents again — triggers oplog replay.
    // The slow agent should still work and get a fast response (server counter > 0).
    let slow_result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_slow,
            "slow_body_stream",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;

    assert_eq!(slow_result2, 2048);

    // The fast agent that previously timed out should also be usable after restart
    let fast_result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_fast,
            "slow_body_stream",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;

    assert_eq!(fast_result2, 2048);

    executor.check_oplog_is_queryable(&worker_id_slow).await?;
    executor.check_oplog_is_queryable(&worker_id_fast).await?;

    http_server.abort();
    drop(executor);
    Ok(())
}

/// Calls slow_body_stream_with_timeout with a very small timeout (1ms) so
/// the timer fires before the HTTP body finishes, then drops/restarts the
/// executor to force oplog replay, and finally calls slow_body_stream
/// (no timeout) to verify the agent recovers correctly. After that initial
/// round-trip, repeats the timeout call in a loop to stress-test replay.
#[test]
#[tracing::instrument]
async fn http_timeout_and_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn({
        async move {
            let route = Router::new().route(
                "/big-byte-array",
                get(move || async move {
                    // Always return a slow streaming body (~10s)
                    let stream = stream::iter(0..100)
                        .throttle(Duration::from_millis(100))
                        .map(|_| Ok::<Bytes, BoxError>(Bytes::from(vec![0u8; 1024])));
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/octet-stream")
                        .body(axum::body::Body::from_stream(stream))
                        .unwrap()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span()
    });

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    let agent_id = {
        use golem_common::phantom_agent_id;
        phantom_agent_id!("StreamingClient", uuid::Uuid::now_v7())
    };

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // 1) Loop of timeout calls
    for i in 0..10 {
        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "slow_body_stream_with_timeout",
                data_value!(0u64),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        info!("Timeout call iteration {i}: {result:?}");
        match &result {
            SchemaValue::Option { inner: None } | SchemaValue::Option { inner: Some(_) } => {}
            other => panic!("expected Option, got {other:?}"),
        }
    }

    // 2) Drop executor and restart to force oplog replay

    tokio::time::sleep(Duration::from_secs(2)).await;

    info!("Dropping executor to force restart...");
    drop(executor);

    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // 3) Single call without timeout — verifies recovery after replay
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "slow_body_stream", data_value!())
        .await?
        .into_typed::<u64>()?;

    info!("Post-restart slow_body_stream result: {result:?}");
    assert_eq!(result, 100 * 1024);

    executor.check_oplog_is_queryable(&worker_id).await?;

    http_server.abort();
    drop(executor);
    Ok(())
}

/// Reproducer for oplog mismatch bug with STREAMING HTTP responses.
///
/// Uses streaming_http_read which reads a chunked HTTP response body
/// chunk by chunk through wstd's async runtime. This produces many
/// interleaved poll/ready/read oplog entries similar to OpenAI streaming.
/// After the first invocation completes, the executor is dropped and
/// restarted to force a full oplog replay. If the replay produces a
/// different sequence of poll/ready calls, we'll see the
/// "expected io::poll::poll, got io::poll::ready" error.
#[test]
#[tracing::instrument]
async fn oplog_replay_after_streaming_http_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 50 chunks with 10ms delay between each — generates ~50 poll/ready cycles
    let (port, server) = streaming_chunk_server(50, Duration::from_millis(10)).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: read a streaming HTTP response with 50 chunks.
    // This builds up a substantial oplog with many subscribe/ready/poll entries.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "streaming_http_read", data_value!())
        .await?
        .into_typed::<String>()?;

    let result_str = result.clone();
    assert!(
        result_str.contains("chunk-0"),
        "Expected streaming response to contain chunk-0, got: {}",
        result_str
    );
    assert!(
        result_str.contains("chunk-49"),
        "Expected streaming response to contain chunk-49, got: {}",
        result_str
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result_str.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor but keep the HTTP server running
    drop(executor);

    // Restart the executor — triggers oplog replay
    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay of the
    // first streaming invocation. If the durable host call sequence diverges
    // during replay, this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "streaming_http_read", data_value!())
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    assert!(
        result2_str.contains("chunk-0"),
        "Expected streaming response to contain chunk-0 in second invocation, got: {}",
        result2_str
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}

/// A transient mid-body failure of a streaming HTTP response must route
/// through worker-level retry and re-issue the recorded request instead of
/// surfacing a truncated body to the guest.
///
/// The server aborts the chunked response body mid-stream on the first
/// attempt and serves the complete body on subsequent attempts. The durable
/// consume-body task classifies the body error as transient, the worker goes
/// to `Retrying`, and the retry's replay finds the consume-body scope
/// incomplete, jumps it to live, and re-issues the recorded request (same
/// idempotency key) — the guest observes only the complete second body.
#[test]
#[tracing::instrument]
async fn http_client_transient_mid_stream_failure_is_retried_and_reissued(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let attempts = Arc::new(AtomicU8::new(0));
    let attempts_clone = attempts.clone();
    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/streaming-chunks",
                get(move || {
                    let attempts = attempts_clone.clone();
                    async move {
                        let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let stream = stream::iter(0..20).map(move |i| {
                            if attempt == 0 && i == 5 {
                                Err::<Bytes, BoxError>("injected mid-body failure".into())
                            } else {
                                Ok(Bytes::from(format!("chunk-{i}\n")))
                            }
                        });

                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "text/plain")
                            .body(axum::body::Body::from_stream(stream))
                            .unwrap()
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "streaming_http_read", data_value!())
        .await?
        .into_typed::<String>()?;

    // The complete second body, with no chunks leaking in from the aborted
    // first attempt (its partial delivery is discarded by the retry's replay).
    let expected: String = (0..20).map(|i| format!("chunk-{i}\n")).collect();
    assert_eq!(
        result, expected,
        "expected exactly the complete re-issued response body"
    );
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the request must have been re-issued exactly once after the mid-body failure"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    http_server.abort();
    drop(executor);
    Ok(())
}

/// Interrupting an agent mid-response-stream and resuming it forces the
/// restart rebuild gate to reissue the recorded P3 send to continue the
/// response stream; the reissued request must carry the recorded request body
/// byte-identically instead of an empty-body reissue.
#[test]
#[tracing::instrument]
async fn http_client_interrupted_mid_response_reissues_recorded_post_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let request_bodies: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_clone = request_bodies.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/",
                post(move |body: Bytes| {
                    let request_bodies = request_bodies_clone.clone();
                    let signal_tx = signal_tx.clone();
                    async move {
                        let is_first = {
                            let mut bodies = request_bodies.lock().unwrap();
                            bodies.push(body.to_vec());
                            bodies.len() == 1
                        };
                        let stream = stream::iter(0..100)
                            .throttle(Duration::from_millis(20))
                            .map(move |i| {
                                if is_first && i == 50 {
                                    let _ = signal_tx.send(());
                                }
                                Ok::<Bytes, BoxError>(Bytes::from(vec![b'x'; 1024]))
                            });

                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "application/octet-stream")
                            .body(axum::body::Body::from_stream(stream))
                            .unwrap()
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient4");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_agent_with_key(
                    &component_clone,
                    &agent_id_clone,
                    &key_clone,
                    "post_with_p3_streamed_body",
                    data_value!(),
                )
                .await;
        }
        .in_current_span(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(5))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "post_with_p3_streamed_body",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let result_value = result.into_typed::<String>()?;
    assert!(
        result_value.starts_with("200 "),
        "Expected a successful 200 response after resume, got: {}",
        &result_value[..result_value.len().min(64)]
    );

    // post_with_p3_streamed_body streams 8 chunks of 8 KiB where byte j of
    // chunk i is (i * 31 + j) % 251
    let expected_body: Vec<u8> = {
        let mut body = Vec::with_capacity(8 * 8 * 1024);
        for i in 0..8usize {
            body.extend((0..8 * 1024usize).map(|j| ((i * 31 + j) % 251) as u8));
        }
        body
    };
    let request_bodies = request_bodies.lock().unwrap();
    assert_eq!(
        request_bodies.len(),
        2,
        "Expected exactly 2 requests (initial attempt + post-resume reissue)"
    );
    assert_eq!(
        request_bodies[0], expected_body,
        "The initial attempt must upload the full body"
    );
    assert_eq!(
        request_bodies[1], expected_body,
        "The rebuilt send must reissue the recorded request body byte-identically, not an empty body"
    );

    Ok(())
}

/// Reproducer for the FutureTrailers non-durable bug (oplog mismatch).
///
/// This test exercises the exact bug mechanism identified in Step 13 of the investigation:
/// 1. Streaming HTTP read using wstd's body.bytes() goes through:
///    stream reads → stream Closed → finish() → FutureTrailers::subscribe() → ready() → get()
/// 2. FutureTrailers::get() is NOT durable because the FutureTrailers handle is never
///    tracked in open_http_requests (the tracking was removed when the stream read returned Closed).
/// 3. During replay, durable ready() returns true from oplog but does NOT call the underlying
///    HostFutureTrailers::ready() — so the state stays Waiting instead of transitioning to Done.
/// 4. Non-durable get() sees Waiting → returns None → guest re-polls, consuming oplog entries
///    meant for the subsequent sleep() call → oplog mismatch.
#[test]
#[tracing::instrument]
async fn oplog_replay_streaming_http_then_sleep_future_trailers_bug(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Use enough chunks to generate substantial oplog entries
    let (port, server) = streaming_chunk_server(20, Duration::from_millis(5)).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: streaming HTTP read (goes through trailers path) then sleep.
    // This creates oplog entries for: stream reads + trailers ready/poll + sleep ready/poll.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "streaming_http_then_sleep",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result_str = result.clone();
    assert!(
        result_str.contains("slept"),
        "Expected result to contain 'slept', got: {}",
        result_str
    );

    info!(
        "First invocation completed: {}, dropping executor to force oplog replay",
        result_str
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor but keep the HTTP server running
    drop(executor);

    // Restart the executor — triggers oplog replay
    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay.
    // Previously this would fail with oplog mismatch because FutureTrailers::get()
    // was non-durable. Now that FutureTrailers tracking is properly maintained
    // through the ownership chain, replay should succeed.
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "streaming_http_then_sleep",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    assert!(
        result2_str.contains("slept"),
        "Expected result to contain 'slept', got: {}",
        result2_str
    );

    server.abort();
    drop(executor);
    Ok(())
}

/// Reproducer for oplog mismatch bug with PARALLEL streaming HTTP reads.
///
/// Runs multiple concurrent streaming HTTP request reads through wstd's reactor,
/// racing them against a timeout. This creates maximum interleaving of
/// subscribe/ready/poll calls across concurrent tasks, which is the closest
/// reproduction of the production OpenAI streaming pattern.
#[test]
#[tracing::instrument]
async fn oplog_replay_after_parallel_streaming_http_reads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 30 chunks with 10ms delay — each request generates ~30 poll/ready cycles
    let (port, server) = streaming_chunk_server(30, Duration::from_millis(10)).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent streaming HTTP reads
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "parallel_streaming_http_reads",
            data_value!(3u64),
        )
        .await?
        .into_typed::<String>()?;

    let result_str = result.clone();
    assert!(
        !result_str.contains("Timeout"),
        "Parallel streaming reads should not have timed out, got: {}",
        result_str
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result_str.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation: triggers full oplog replay of the parallel streaming invocation
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "parallel_streaming_http_reads",
            data_value!(3u64),
        )
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    assert!(
        !result2_str.contains("Timeout"),
        "Second invocation should not have timed out, got: {}",
        result2_str
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}

/// Reproducer for the oplog mismatch bug with raw WASI HTTP streaming.
///
/// This test mimics the production pattern from wasm-rquickjs/golem-wasi-http:
/// - Uses raw WASI HTTP APIs to send a request
/// - Reads the response body with subscribe() + AsyncPollable::wait_for() + read()
/// - Drops the stream and body WITHOUT calling incoming_body.finish()
///
/// This is the exact pattern that the production component uses, which differs
/// from wstd's body.bytes() (which goes through the full trailers path).
#[test]
#[tracing::instrument]
async fn oplog_replay_after_raw_streaming_http_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 50 chunks with 10ms delay between each
    let (port, server) = streaming_chunk_server(50, Duration::from_millis(10)).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: read a streaming HTTP response using raw WASI APIs
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "raw_streaming_http_read",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result_str = result.clone();
    assert!(
        result_str.contains("chunk-0"),
        "Expected streaming response to contain chunk-0, got: {}",
        result_str
    );
    assert!(
        result_str.contains("chunk-49"),
        "Expected streaming response to contain chunk-49, got: {}",
        result_str
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result_str.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor but keep the HTTP server running
    drop(executor);

    // Restart the executor — triggers oplog replay
    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay.
    // If the durable host call sequence diverges during replay,
    // this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "raw_streaming_http_read",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    assert!(
        result2_str.contains("chunk-0"),
        "Expected streaming response to contain chunk-0 in second invocation, got: {}",
        result2_str
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}

/// Reproducer for oplog mismatch bug with PARALLEL raw WASI HTTP streaming reads.
///
/// This is the closest reproduction of the production wasm-rquickjs/golem-wasi-http
/// pattern: multiple concurrent raw WASI HTTP streaming reads running inside wstd's
/// reactor. Each read uses subscribe() + AsyncPollable::wait_for() + read() in a
/// loop and drops the stream/body without calling finish(). The concurrency
/// creates maximum interleaving of nonblock_check_pollables/block_on_pollables
/// (io::poll::poll) with WaitFor::poll ready() calls (io::poll::ready).
#[test]
#[tracing::instrument]
async fn oplog_replay_after_parallel_raw_streaming_http_reads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 30 chunks with 10ms delay — each request generates many poll/ready cycles
    let (port, server) = streaming_chunk_server(30, Duration::from_millis(10)).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent raw streaming HTTP reads, each doing 3 sequential requests
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "parallel_raw_streaming_http_reads",
            data_value!(3u64),
        )
        .await?
        .into_typed::<String>()?;

    let result_str = result.clone();
    assert!(
        !result_str.contains("Timeout"),
        "Parallel raw streaming reads should not have timed out, got: {}",
        result_str
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result_str.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    info!("Restarting executor...");
    let executor = start(deps, &context).await?;
    info!("Executor restarted");

    // Second invocation: triggers full oplog replay of the parallel raw streaming invocation.
    // If the durable host call sequence diverges during replay,
    // this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "parallel_raw_streaming_http_reads",
            data_value!(3u64),
        )
        .await?
        .into_typed::<String>()?;

    let result2_str = result2.clone();
    assert!(
        !result2_str.contains("Timeout"),
        "Second invocation should not have timed out, got: {}",
        result2_str
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);
    Ok(())
}
