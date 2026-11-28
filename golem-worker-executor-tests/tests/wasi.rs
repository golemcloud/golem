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
use anyhow::{Context as _, anyhow};
use axum::body::Body;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{BoxError, Router};
use bytes::Bytes;
use futures::stream;
use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
use golem_api_grpc::proto::golem::worker::{LogEvent, log_event};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
};
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentFileSystemNode, AgentFileSystemNodeKind, AgentTypeName,
    ParsedAgentKey, RevertToOplogIndex, RevertWorkerTarget,
};
use golem_common::model::component::{
    AgentFilePermissions, AgentTypeProvisionConfigUpdate, CanonicalFilePath, ComponentDto,
};
use golem_common::model::filesystem::{FileByteSelection, FileReadHead};
use golem_common::model::oplog::{
    HostRequest, HostRequestMonotonicClockDuration, OplogIndex, PublicAgentInvocation,
    PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId, PromiseId};
use golem_common::schema::FromSchema;
#[cfg(target_os = "linux")]
use golem_common::schema::{SchemaValue, schema_value::ResultValuePayload};
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::{ComponentFileSystemNodeDetails, GetFileSystemNodeResult};
use golem_test_framework::dsl::{
    AgentResult, TestDsl, count_agent_invocation_pair_since, drain_connection, stderr_events,
    within,
};
use golem_test_framework::model::IFSEntry;
use golem_worker_executor::services::golem_config::{
    FilesystemStorageMode, GolemConfig, HttpClientConfig, HttpClientEnabledConfig, SnapshotPolicy,
};
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::assert_worker_error;
use golem_worker_executor_test_utils::{
    ExecutorTest, LastUniqueId, LocalHttpServer, OplogView, PrecompiledComponent,
    ReplayAdmissionStage, Teardown, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_with_http_client_config, start_with_overrides,
};
use http::{HeaderMap, StatusCode};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::spawn;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tracing::{Instrument, debug, info};

/// The shard manager process every shard push in these tests names.
const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000001";

/// How long a test waits for streamed stdout or stderr events.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a test waits for a worker to reach an expected status or unload.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a short status change, such as starting to run, may take.
const SHORT_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a resumed worker may take to finish its work and become idle.
const LONG_STATUS_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a sleeping worker may take to suspend.
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a suspended worker may take to unload.
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(5);
/// The upper bound of recovery after a recorded deadline has expired; a fresh timer would exceed it.
const RECORDED_DEADLINE_RECOVERY: Duration = Duration::from_secs(4);
/// How long a test waits for a shard revocation to stop its workers.
const SHARD_REVOKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an API call on an unloaded worker may take to restore it.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(30);
/// The pause between two polls of a worker or oplog condition.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a test lets an invocation run before it interrupts the parked call.
const PARK_DELAY: Duration = Duration::from_secs(2);
/// How long a status change after an interrupt or resume hint may take.
const HINT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a worker may take to recover after an executor restart.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a test waits for a guest connection or a parked network call.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the UDP test resends its datagram while the repaired receive waits.
const DATAGRAM_INTERVAL: Duration = Duration::from_millis(10);
/// How long a test listens to prove that no further datagram arrives.
const SILENCE_WINDOW: Duration = Duration::from_millis(100);

const UDP_RECEIVE: &str = "sockets::types::udp-socket::receive";
const UDP_SEND: &str = "sockets::types::udp-socket::send";
const TCP_RECEIVE: &str = "sockets::types::tcp-socket::receive";
const TCP_RECEIVE_CHUNK: &str = "sockets::types::tcp-socket::receive-chunk";

/// `clocks::StdTimeApisResult` of the host-api-tests component.
#[derive(Debug, FromSchema)]
struct StdTimeApisResult {
    elapsed1: f64,
    elapsed2: f64,
    odt: String,
}

/// `file_system::FileWriteReadDeleteResult` of the host-api-tests component.
#[derive(Debug, PartialEq, FromSchema)]
struct FileWriteReadDeleteResult {
    read_nonexisting: Option<String>,
    read_existing: Option<String>,
    read_after_delete: Option<String>,
}

/// `file_system::DirEntry` of the host-api-tests component.
#[derive(Debug, PartialEq, FromSchema)]
struct DirEntry {
    name: String,
    is_dir: bool,
}

/// `file_system::DirectoriesResult` of the host-api-tests component.
#[derive(Debug, PartialEq, FromSchema)]
struct DirectoriesResult {
    count1: u32,
    root_entries: Vec<DirEntry>,
    test_entries: Vec<DirEntry>,
    count2: u32,
}

/// `file_system::FileTimestamps` of the host-api-tests component.
#[derive(Debug, PartialEq, FromSchema)]
struct FileTimestamps {
    modified_secs: u64,
    modified_nanos: u32,
    accessed_secs: u64,
    accessed_nanos: u32,
}

/// `file_system::HashResult` of the host-api-tests component.
#[derive(Debug, PartialEq, FromSchema)]
struct HashResult {
    upper: u64,
    lower: u64,
}

/// The five reads that `FileReadWrite::run` of the initial-file-system component returns.
type FileReadWriteResult = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn dir_entry(name: &str, is_dir: bool) -> DirEntry {
    DirEntry {
        name: name.to_string(),
        is_dir,
    }
}

/// An initial file from `initial-file-system/files/<source>` mounted at the absolute `target`.
fn initial_file(source: &str, target: &str, permissions: AgentFilePermissions) -> IFSEntry {
    IFSEntry {
        source_path: PathBuf::from("initial-file-system/files").join(source),
        target_path: CanonicalFilePath::from_abs_str(target).unwrap(),
        permissions,
    }
}

/// The file system nodes at `path`, sorted by name, with `last_modified` zeroed for comparison.
async fn listed_nodes(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    path: &str,
) -> anyhow::Result<Vec<AgentFileSystemNode>> {
    let mut nodes = executor
        .get_file_system_node(worker_id, path)
        .await?
        .into_iter()
        .map(|node| AgentFileSystemNode {
            last_modified: 0,
            ..node
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.name.clone());
    Ok(nodes)
}

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

fn assert_interrupted(failure: &anyhow::Error) {
    assert_worker_error!(
        *failure,
        WorkerExecutorError::Runtime { details } if details == "Interrupted via the Golem API",
        "the invocation fails with the interruption"
    );
}

fn count_entries(
    oplog: &[PublicOplogEntryWithIndex],
    predicate: impl Fn(&PublicOplogEntry) -> bool,
) -> usize {
    oplog.iter().filter(|entry| predicate(&entry.entry)).count()
}

fn is_interrupted(entry: &PublicOplogEntry) -> bool {
    matches!(entry, PublicOplogEntry::Interrupted(_))
}

fn is_invocation_finished(entry: &PublicOplogEntry) -> bool {
    matches!(entry, PublicOplogEntry::AgentInvocationFinished(_))
}

/// The `Start`s after `after` whose host function name `name_matches` and that have no `End`
/// or `Cancelled`.
fn incomplete_starts(
    oplog: &[PublicOplogEntryWithIndex],
    after: OplogIndex,
    name_matches: impl Fn(&str) -> bool,
) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start) if name_matches(&start.function_name))
        })
        .map(|entry| entry.oplog_index)
        .filter(|start| oplog.call(*start).terminal_count() == 0)
        .collect()
}

/// How far an interrupted durable wait and the invocations after `after` got.
#[derive(Debug, PartialEq)]
struct WaitProgress {
    wait_starts: usize,
    wait_ends: usize,
    invocations_started: usize,
    invocations_finished: usize,
}

impl WaitProgress {
    fn of(oplog: &[PublicOplogEntryWithIndex], wait_start: OplogIndex, after: OplogIndex) -> Self {
        let wait = oplog.call(wait_start);
        let later = || oplog.iter().filter(|entry| entry.oplog_index > after);
        Self {
            wait_starts: usize::from(wait.start.is_some()),
            wait_ends: wait.ends.len(),
            invocations_started: later()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
                .count(),
            invocations_finished: later()
                .filter(|entry| is_invocation_finished(&entry.entry))
                .count(),
        }
    }
}

/// The `AgentInvocationStarted` and `AgentInvocationFinished` entries after `after` of the
/// method invocation with this ID and method name.
fn method_invocation_progress(
    oplog: &[PublicOplogEntryWithIndex],
    after: OplogIndex,
    invocation_id: &InvocationId,
    method_name: &str,
) -> (usize, usize) {
    let mut current_is_method = false;
    let mut admitted = 0;
    let mut finished = 0;
    for entry in oplog.iter().filter(|entry| entry.oplog_index > after) {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current_is_method = matches!(&start.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                    if &method.invocation_id == invocation_id && method.method_name == method_name);
                admitted += usize::from(current_is_method);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current_is_method => {
                finished += 1;
            }
            _ => {}
        }
    }
    (admitted, finished)
}

/// A TCP server that never answers its first connection and reports when the peer closes it.
/// It answers every later connection with `hello` and closes it.
struct SilentFirstTcpServer {
    port: u16,
    /// Receives when the first connection is accepted.
    connected: tokio::sync::mpsc::UnboundedReceiver<()>,
    /// Receives whether the peer closed the first connection.
    closed: tokio::sync::mpsc::UnboundedReceiver<bool>,
    accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl SilentFirstTcpServer {
    async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (connected_tx, connected) = tokio::sync::mpsc::unbounded_channel();
        let (closed_tx, closed) = tokio::sync::mpsc::unbounded_channel();
        let accepted = Arc::new(AtomicUsize::new(0));
        let server_accepted = accepted.clone();
        let task = spawn(
            async move {
                let mut first = true;
                while let Ok((mut stream, _)) = listener.accept().await {
                    server_accepted.fetch_add(1, Ordering::SeqCst);
                    let is_first = first;
                    first = false;
                    let connected_tx = connected_tx.clone();
                    let closed_tx = closed_tx.clone();
                    spawn(async move {
                        if is_first {
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
        Ok(Self {
            port,
            connected,
            closed,
            accepted,
            task,
        })
    }
}

/// The strings of a returned list value; the Linux tests in `wasi/linux.rs` use it.
#[cfg(target_os = "linux")]
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

fn string_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A string-valued agent config entry at the single-segment `key`.
fn config_entry(key: &str, value: &str) -> AgentConfigEntryDto {
    AgentConfigEntryDto {
        path: vec![key.to_string()],
        value: serde_json::Value::String(value.to_string()).into(),
    }
}

async fn assert_reconstructed_writable_file(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentKey,
) -> anyhow::Result<()> {
    let result: Vec<String> = executor
        .invoke_and_await_typed(component, agent_id, "inspect_writable", data_value!())
        .await?;
    assert_eq!(
        result,
        vec![
            "p2_read=p3-to-p2".to_string(),
            "p3_read=p3-to-p2".to_string(),
        ]
    );
    Ok(())
}

fn full_replay_config(config: &mut GolemConfig) {
    config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
    config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
}

#[test]
#[tracing::instrument]
async fn write_stdout(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Logging", "write-stdout-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    for _ in 0..2 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "write_stdout", data_value!())
            .await?;
    }

    let mut stdout = vec![];
    let start_time = Instant::now();
    while stdout.len() < 2 && start_time.elapsed() < OUTPUT_TIMEOUT {
        match tokio::time::timeout(OUTPUT_TIMEOUT, rx.recv()).await {
            Ok(Some(LogEvent {
                event: Some(log_event::Event::Stdout(event)),
            })) => stdout.push(event),
            Ok(Some(_)) => {}
            _ => break,
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Each output event names the invocation that wrote it.
    let started = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter_map(|entry| match entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match params.invocation {
                PublicAgentInvocation::AgentMethodInvocation(call)
                    if call.method_name == "write_stdout" =>
                {
                    Some(call.invocation_id.0)
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 2, "two write_stdout invocations started");
    assert_ne!(
        started[0], started[1],
        "the two invocations have their own IDs"
    );
    assert_eq!(
        stdout
            .iter()
            .map(|event| event.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Sample text written to the output\n"; 2]
    );
    assert_eq!(
        stdout
            .iter()
            .map(|event| event.invocation_id.map(uuid::Uuid::from))
            .collect::<Vec<_>>(),
        started.into_iter().map(Some).collect::<Vec<_>>(),
        "each stdout event names the invocation that wrote it"
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Logging", "write-stderr-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "write_stderr", data_value!())
        .await?;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 4 && start_time.elapsed() < OUTPUT_TIMEOUT {
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Io", "read-stdin-1");
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clocks", "clocks-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let times: StdTimeApisResult = executor
        .invoke_and_await_typed(&component, &agent_id, "use_std_time_apis", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let epoch_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let diff1 = (epoch_seconds - times.elapsed1).abs();
    let parsed_odt = str::parse::<jiff::Timestamp>(times.odt.as_str()).unwrap();
    let odt_diff = epoch_seconds - parsed_odt.as_second() as f64;

    assert!(diff1 < 5.0, "the wall clock is within 5 s of the host's");
    assert!(
        (2.0..3.0).contains(&times.elapsed2),
        "the monotonic clock measures the guest's 2 s sleep: {}",
        times.elapsed2
    );
    assert!(
        odt_diff < 5.0,
        "the formatted time is within 5 s of the host's"
    );

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
    let agent_id = agent_key!("FileSystem", "file-write-read-delete-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let reads: FileWriteReadDeleteResult = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "run_file_write_read_delete",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        reads,
        FileWriteReadDeleteResult {
            read_nonexisting: None,
            read_existing: Some("hello world".to_string()),
            read_after_delete: None,
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                initial_file("foo.txt", "/foo.txt", AgentFilePermissions::ReadOnly),
                initial_file("baz.txt", "/bar/baz.txt", AgentFilePermissions::ReadWrite),
            ],
        )
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_key!("FileReadWrite", "initial-file-read-write-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let reads: FileReadWriteResult = executor
        .invoke_and_await_typed(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        reads,
        (
            Some("foo\n".to_string()),
            None,
            None,
            Some("baz\n".to_string()),
            Some("hello world".to_string()),
        )
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
    initial_file_p3_parity_impl(last_unique_id, deps, initial_file_system).await
}

#[test]
#[tracing::instrument]
async fn initial_file_directory_hard_link_gives_not_permitted_through_p2_and_p3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, initial_file_system).await?;
    let agent_id = agent_key!("P3FileSystem", "directory-hard-link");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let linked: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "run_directory_link", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_replay_target",
            data_value!("written after the refused links".to_string()),
        )
        .await?;
    let inspected: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("replay-target.txt".to_string()),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        linked,
        [
            "directory_create_p2=ok",
            "directory_link_p2=err:not-permitted",
            "create_after_directory_link_p2=ok",
            "directory_create_p3=ok",
            "directory_link_p3=err:not-permitted",
            "create_after_directory_link_p3=ok",
        ]
        .map(String::from)
    );
    assert_eq!(
        inspected,
        [
            "p2_read=written after the refused links",
            "p3_read=written after the refused links",
        ]
        .map(String::from)
    );
    Ok(())
}

/// The guest's report of `P3FileSystem::run`: one `check=outcome` line per P2/P3 file operation.
const P3_PARITY_REPORT: &[&str] = &[
    "ro_flags_p2_write=false",
    "ro_flags_p3_write=false",
    "ro_hash_parity=true",
    "ro_hash_p3_deterministic=true",
    "ro_hash_at_parity=true",
    "ro_set_times_p2=err:not-permitted",
    "ro_set_times_p3=err:not-permitted",
    "ro_set_times_at_p2=err:not-directory",
    "ro_set_times_at_p3=err:not-directory",
    "ro_rename_at_p2=err:not-directory",
    "ro_rename_at_p3=err:not-directory",
    "ro_symlink_at_p2=err:not-directory",
    "ro_symlink_at_p3=err:not-directory",
    "ro_unlink_file_at_p2=err:not-directory",
    "ro_unlink_file_at_p3=err:not-directory",
    "ro_parent_open_write_p2=err:not-permitted",
    "ro_parent_open_write_p3=err:not-permitted",
    "ro_invalid_flags_p2=err:unsupported",
    "ro_invalid_flags_p3=err:unsupported",
    "ro_unlink_p2=ok",
    "ro_rename_p2=ok",
    "ro_link_p2=ok",
    "ro_directory_move_p2=ok",
    "ro_installed_open_write_p2=err:not-permitted",
    "ro_installed_truncate_p2=err:not-permitted",
    "ro_installed_set_times_at_p2=err:not-permitted",
    "ro_installed_set_size_p2=err:not-permitted",
    "ro_installed_set_times_p2=err:not-permitted",
    "ro_installed_flags_write_p2=false",
    "ro_renamed_open_write_p2=err:not-permitted",
    "ro_renamed_truncate_p2=err:not-permitted",
    "ro_renamed_set_times_at_p2=err:not-permitted",
    "ro_renamed_set_size_p2=err:not-permitted",
    "ro_renamed_set_times_p2=err:not-permitted",
    "ro_renamed_flags_write_p2=false",
    "ro_linked_open_write_p2=err:not-permitted",
    "ro_linked_truncate_p2=err:not-permitted",
    "ro_linked_set_times_at_p2=err:not-permitted",
    "ro_linked_set_size_p2=err:not-permitted",
    "ro_linked_set_times_p2=err:not-permitted",
    "ro_linked_flags_write_p2=false",
    "ro_moved_open_write_p2=err:not-permitted",
    "ro_moved_truncate_p2=err:not-permitted",
    "ro_moved_set_times_at_p2=err:not-permitted",
    "ro_moved_set_size_p2=err:not-permitted",
    "ro_moved_set_times_p2=err:not-permitted",
    "ro_moved_flags_write_p2=false",
    "rw_write_after_directory_move_p2=ok",
    "create_after_directory_move_p2=ok",
    "ro_unlink_p3=ok",
    "ro_rename_p3=ok",
    "ro_link_p3=ok",
    "ro_directory_move_p3=ok",
    "ro_installed_open_write_p3=err:not-permitted",
    "ro_installed_truncate_p3=err:not-permitted",
    "ro_installed_set_times_at_p3=err:not-permitted",
    "ro_installed_set_size_p3=err:not-permitted",
    "ro_installed_set_times_p3=err:not-permitted",
    "ro_installed_flags_write_p3=false",
    "ro_renamed_open_write_p3=err:not-permitted",
    "ro_renamed_truncate_p3=err:not-permitted",
    "ro_renamed_set_times_at_p3=err:not-permitted",
    "ro_renamed_set_size_p3=err:not-permitted",
    "ro_renamed_set_times_p3=err:not-permitted",
    "ro_renamed_flags_write_p3=false",
    "ro_linked_open_write_p3=err:not-permitted",
    "ro_linked_truncate_p3=err:not-permitted",
    "ro_linked_set_times_at_p3=err:not-permitted",
    "ro_linked_set_size_p3=err:not-permitted",
    "ro_linked_set_times_p3=err:not-permitted",
    "ro_linked_flags_write_p3=false",
    "ro_moved_open_write_p3=err:not-permitted",
    "ro_moved_truncate_p3=err:not-permitted",
    "ro_moved_set_times_at_p3=err:not-permitted",
    "ro_moved_set_size_p3=err:not-permitted",
    "ro_moved_set_times_p3=err:not-permitted",
    "ro_moved_flags_write_p3=false",
    "rw_write_after_directory_move_p3=ok",
    "create_after_directory_move_p3=ok",
    "ro_alias_create_p2=ok",
    "ro_alias_open_write_p2=err:not-permitted",
    "ro_alias_unlink_p2=ok",
    "ro_alias_create_p3=ok",
    "ro_alias_open_write_p3=err:not-permitted",
    "ro_alias_unlink_p3=ok",
    "rw_flags_p2_write=true",
    "rw_flags_p3_write=true",
    "rw_hash_parity=true",
    "rw_set_times_p2=ok",
    "rw_set_times_p3=ok",
    "p2_write_p3_read=p2-to-p3",
    "p3_write_p2_read=p3-to-p2",
];

async fn initial_file_p3_parity_impl(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[
                initial_file("foo.txt", "/foo.txt", AgentFilePermissions::ReadOnly),
                initial_file("foo.txt", "/foo-copy.txt", AgentFilePermissions::ReadOnly),
                initial_file("baz.txt", "/bar/baz.txt", AgentFilePermissions::ReadWrite),
            ]
            .into_iter()
            .chain(["p2", "p3"].into_iter().flat_map(|preview| {
                [
                    ("unlink.txt", AgentFilePermissions::ReadOnly),
                    ("rename.txt", AgentFilePermissions::ReadOnly),
                    ("link.txt", AgentFilePermissions::ReadOnly),
                    ("dir/inner.txt", AgentFilePermissions::ReadOnly),
                    ("writable.txt", AgentFilePermissions::ReadWrite),
                ]
                .map(|(name, permissions)| {
                    initial_file("foo.txt", &format!("/{preview}/{name}"), permissions)
                })
            }))
            .collect::<Vec<_>>(),
        )
        .store()
        .await?;

    let agent_id = agent_key!("P3FileSystem", "initial-file-p3-parity-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let abandoned_completion: bool = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "abandon_p3_write_completion",
            data_value!(),
        )
        .await?;
    assert!(
        abandoned_completion,
        "the guest abandons a pending P3 write completion"
    );

    let result: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result, P3_PARITY_REPORT);

    // Crash the worker so the next invocation replays the recorded oplog first,
    // verifying the P3 metadata-hash -> durable stat call sequence is replay-stable
    // and that replay reconstructed the mutable initial file without help from
    // the verification invocation.
    executor.simulated_crash(&worker_id).await?;

    let result_after_crash: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "inspect_run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result_after_crash,
        vec![
            "p2_read=p3-to-p2".to_string(),
            "p3_read=p3-to-p2".to_string(),
        ]
    );

    Ok(())
}

/// Runs one `FilesystemTree` operation and returns the guest's answer (`"ok"` on success).
async fn filesystem_tree_apply(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentKey,
    operation: &str,
    path: &str,
    argument: &str,
) -> anyhow::Result<String> {
    let (operation, path, argument) = (
        operation.to_string(),
        path.to_string(),
        argument.to_string(),
    );
    executor
        .invoke_and_await_typed(
            component,
            agent,
            "apply",
            data_value!(operation, path, argument),
        )
        .await
}

/// One line per node of the agent's file tree, as `FilesystemTree::describe` lists them.
async fn filesystem_tree_describe(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentKey,
) -> anyhow::Result<Vec<String>> {
    executor
        .invoke_and_await_typed(component, agent, "describe", data_value!())
        .await
}

async fn start_on_empty_root_without_snapshots(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    root: &Path,
) -> anyhow::Result<TestWorkerExecutor> {
    let root = root.to_path_buf();
    let overrides = TestExecutorOverrides::configured(move |config| {
        config.filesystem_storage.mode = FilesystemStorageMode::Directory {
            root: root.clone().into(),
        };
        config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
        config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
    });
    start_with_overrides(deps, context, overrides).await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn full_replay_on_an_empty_root_rebuilds_the_tree_after_initial_file_operations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let first_root = tempfile::tempdir()?;
    let second_root = tempfile::tempdir()?;
    let executor = start_on_empty_root_without_snapshots(deps, &context, first_root.path()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FilesystemTree",
            &[
                initial_file("foo.txt", "/ro-kept.txt", AgentFilePermissions::ReadOnly),
                initial_file("foo.txt", "/ro-deleted.txt", AgentFilePermissions::ReadOnly),
                initial_file("foo.txt", "/ro-renamed.txt", AgentFilePermissions::ReadOnly),
                initial_file("bar.txt", "/ro-linked.txt", AgentFilePermissions::ReadOnly),
                initial_file(
                    "bar.txt",
                    "/dir/ro-in-dir.txt",
                    AgentFilePermissions::ReadOnly,
                ),
                initial_file(
                    "baz.txt",
                    "/rw-modified.txt",
                    AgentFilePermissions::ReadWrite,
                ),
                initial_file(
                    "baz.txt",
                    "/rw-deleted.txt",
                    AgentFilePermissions::ReadWrite,
                ),
            ],
        )
        .store()
        .await?;
    let agent = agent_key!("FilesystemTree", "full-replay");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    for (operation, path, argument) in [
        ("remove", "ro-deleted.txt", ""),
        ("write", "rw-modified.txt", "modified by the agent"),
        ("remove", "rw-deleted.txt", ""),
        ("rename", "ro-renamed.txt", "renamed.txt"),
        ("link", "ro-linked.txt", "second-name.txt"),
        ("rename", "dir", "moved-dir"),
        ("write", "agent.txt", "agent data"),
        ("link", "agent.txt", "agent-alias.txt"),
    ] {
        let answer =
            filesystem_tree_apply(&executor, &component, &agent, operation, path, argument).await?;
        assert_eq!(answer, "ok", "{operation} {path} {argument}");
    }
    let live = filesystem_tree_describe(&executor, &component, &agent).await?;
    assert_eq!(
        live,
        [
            r#"agent-alias.txt file links=2 writable=true content="agent data""#,
            r#"agent.txt file links=2 writable=true content="agent data""#,
            "moved-dir dir",
            r#"moved-dir/ro-in-dir.txt file links=1 writable=false content="bar\n""#,
            r#"renamed.txt file links=1 writable=false content="foo\n""#,
            r#"ro-kept.txt file links=1 writable=false content="foo\n""#,
            r#"ro-linked.txt file links=2 writable=false content="bar\n""#,
            r#"rw-modified.txt file links=1 writable=true content="modified by the agent""#,
            r#"second-name.txt file links=2 writable=false content="bar\n""#,
        ]
        .map(String::from)
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_))),
        "the oplog holds a snapshot, so the restart could skip part of the replay"
    );
    drop(executor);

    let restarted =
        start_on_empty_root_without_snapshots(deps, &context, second_root.path()).await?;
    let replayed = filesystem_tree_describe(&restarted, &component, &agent).await?;

    assert_eq!(replayed, live);
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
    filesystem_mutation_histories_reconstruct_impl(last_unique_id, deps, initial_file_system).await
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, initial_file_system).await?;
    let agent_id = agent_key!("P3FileSystem", "cross-preview-append");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let coordinated: bool = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "run_cross_preview_append",
            data_value!(),
        )
        .await?;
    assert!(coordinated);

    executor.simulated_crash(&worker_id).await?;
    let reconstructed: bool = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_cross_preview_append",
            data_value!(),
        )
        .await?;
    assert!(reconstructed);
    Ok(())
}

async fn filesystem_mutation_histories_reconstruct_impl(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        initial_file_system,
        TestExecutorOverrides::configured(full_replay_config),
    )
    .await?;
    let agent_id = agent_key!("P3FileSystem", "filesystem-reconstruction-matrix");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let expected: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "run_reconstruction_matrix",
            data_value!(),
        )
        .await?;
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
    let reconstructed: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_reconstruction_matrix",
            data_value!(),
        )
        .await?;
    assert_eq!(reconstructed, expected);
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
    filesystem_reconstruction_stops_at_exact_revert_target_impl(
        last_unique_id,
        deps,
        initial_file_system,
    )
    .await
}

async fn filesystem_reconstruction_stops_at_exact_revert_target_impl(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        initial_file_system,
        TestExecutorOverrides::configured(full_replay_config),
    )
    .await?;
    let agent_id = agent_key!("P3FileSystem", "filesystem-exact-replay-target");
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
    let result: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("replay-target.txt"),
        )
        .await?;
    assert_eq!(result, vec!["p2_read=first", "p3_read=first"]);
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
    filesystem_reconstruction_uses_updated_initial_files_impl(
        last_unique_id,
        deps,
        initial_file_system,
    )
    .await
}

async fn filesystem_reconstruction_uses_updated_initial_files_impl(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides::configured(full_replay_config),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[initial_file(
                "baz.txt",
                "/versioned.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .store()
        .await?;
    let agent_id = agent_key!("P3FileSystem", "filesystem-updated-initial-files");
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
            vec![initial_file(
                "foo.txt",
                "/versioned.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .await?;
    executor
        .auto_update_worker(&worker_id, updated.revision, false)
        .await?;

    let updated_result: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("versioned.txt"),
        )
        .await?;
    assert_eq!(updated_result, vec!["p2_read=foo\n", "p3_read=foo\n"]);

    executor.simulated_crash(&worker_id).await?;
    let reconstructed: Vec<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "inspect_path",
            data_value!("versioned.txt"),
        )
        .await?;
    assert_eq!(reconstructed, vec!["p2_read=foo\n", "p3_read=foo\n"]);
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
    filesystem_full_replay_survives_lifecycle_transitions_impl(
        last_unique_id,
        deps,
        initial_file_system,
    )
    .await
}

async fn filesystem_full_replay_survives_lifecycle_transitions_impl(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let first_root = tempfile::tempdir()?;
    let second_root = tempfile::tempdir()?;
    let executor = start_on_empty_root_without_snapshots(deps, &context, first_root.path()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_key!("P3FileSystem", "filesystem-full-replay-lifecycles");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let owned_agent_id = worker_id.clone();

    let result: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "run_writable", data_value!())
        .await?;
    assert_eq!(
        result,
        vec![
            "p2_write_p3_read=p2-to-p3".to_string(),
            "p3_write_p2_read=p3-to-p2".to_string(),
        ]
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, STATUS_TIMEOUT)
        .await?;
    within(
        STATUS_TIMEOUT,
        "the worker to become loaded-idle after its billing window closed",
        async {
            while executor.worker_eviction_class(&owned_agent_id).await
                != Some(EvictionClass::LoadedIdle)
            {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        },
    )
    .await?;
    assert!(executor.stop_worker_if_idle(&owned_agent_id).await?);
    assert!(!executor.worker_is_loaded(&owned_agent_id).await);
    assert_reconstructed_writable_file(&executor, &component, &agent_id).await?;

    let shard = ShardId { value: 0 };
    let mut client = executor.client.clone();
    let revoked = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![shard],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    within(
        SHARD_REVOKE_TIMEOUT,
        "the worker to unload after its shard was revoked",
        async {
            while executor.worker_is_loaded(&owned_agent_id).await {
                tokio::task::yield_now().await;
            }
        },
    )
    .await?;
    assert!(!executor.worker_is_loaded(&owned_agent_id).await);
    let assigned = client
        .assign_shards(AssignShardsRequest {
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(shard),
                epoch: 0,
            }],
            // A lease TTL is required on the wire; one long enough that this
            // round trip does not depend on timing.
            revision: 1,
            number_of_shards: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
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

    let relocated =
        start_on_empty_root_without_snapshots(deps, &context, second_root.path()).await?;
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                initial_file("foo.txt", "/foo.txt", AgentFilePermissions::ReadOnly),
                initial_file("baz.txt", "/bar/baz.txt", AgentFilePermissions::ReadWrite),
                initial_file("baz.txt", "/baz.txt", AgentFilePermissions::ReadWrite),
            ],
        )
        .store()
        .await?;

    let agent_id = agent_key!("FileReadWrite", "initial-file-listing-1");
    let worker_id = executor.start_agent(&component.id, agent_id).await?;

    let result = listed_nodes(&executor, &worker_id, "/").await?;
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

    let result = listed_nodes(&executor, &worker_id, "/bar").await?;
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

    let result = listed_nodes(&executor, &worker_id, "/baz.txt").await?;
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
#[timeout("120s")]
#[tracing::instrument]
async fn initial_file_reading_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "FileReadWrite",
            &[
                initial_file("foo.txt", "/foo.txt", AgentFilePermissions::ReadOnly),
                initial_file("baz.txt", "/bar/baz.txt", AgentFilePermissions::ReadWrite),
            ],
        )
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_key!("FileReadWrite", "initial-file-read-write-3");
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

    // Retain the lookup result across unload: intake must restore through this handle,
    // replaying writable contents without starting a new invocation.
    let owned_agent_id = worker_id.clone();
    let worker = executor
        .active_agent(&owned_agent_id)
        .await
        .expect("active agent")
        .primary();
    let before_index = executor.oplog_max_index(&worker_id).await?;

    for read_contents in [true, false] {
        within(STATUS_TIMEOUT, "the idle worker to unload", async {
            while !worker.stop_if_idle().await {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await?;
        assert!(!executor.worker_is_loaded(&owned_agent_id).await);

        if read_contents {
            let result = within(
                RESTORE_TIMEOUT,
                "the read through the unloaded handle to restore the worker",
                worker.read_file(
                    CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    FileByteSelection::Full,
                ),
            )
            .await??;
            assert!(matches!(result.head, FileReadHead::File(_)));
            let mut body = result.body;
            let mut contents = Vec::new();
            while let Some(chunk) = body.next().await {
                contents.extend_from_slice(&chunk?);
            }
            assert_eq!(contents, b"hello world");
        } else {
            let result = within(
                RESTORE_TIMEOUT,
                "the listing through the unloaded handle to restore the worker",
                worker.get_file_system_node(CanonicalFilePath::from_abs_str("/bar").unwrap()),
            )
            .await??;
            let GetFileSystemNodeResult::Ok(nodes) = result else {
                panic!("expected restored directory");
            };
            assert_eq!(
                nodes
                    .iter()
                    .map(|node| (node.name.as_str(), &node.details))
                    .collect::<Vec<_>>(),
                vec![(
                    "baz.txt",
                    &ComponentFileSystemNodeDetails::File {
                        permissions: AgentFilePermissions::ReadWrite,
                        size: 11,
                    }
                )],
                "the restored listing shows the written file"
            );
        }
    }

    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&after, before_index),
        (0, 0),
        "restoring through the handle starts no invocation"
    );

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result: DirectoriesResult = executor
        .invoke_and_await_typed(&component, &agent_id, "run_directories", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        DirectoriesResult {
            count1: 0,
            root_entries: vec![dir_entry("/test", true)],
            test_entries: vec![
                dir_entry("/test/dir1", true),
                dir_entry("/test/dir2", true),
                dir_entry("/test/hello.txt", false),
            ],
            // This should be 0 if remove_directory worked.
            count2: 1,
        }
    );

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result: DirectoriesResult = executor
        .invoke_and_await_typed(&component, &agent_id, "run_directories", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    // NOTE: if the directory listing would not be stable, replay would fail with divergence error

    let metadata = executor
        .wait_for_status(&worker_id, AgentStatus::Idle, SHORT_STATUS_TIMEOUT)
        .await?;

    assert_eq!(metadata.status, AgentStatus::Idle);

    assert_eq!(
        result,
        DirectoriesResult {
            count1: 0,
            root_entries: vec![dir_entry("/test", true)],
            test_entries: vec![
                dir_entry("/test/dir1", true),
                dir_entry("/test/dir2", true),
                dir_entry("/test/hello.txt", false),
            ],
            // This should be 0 if remove_directory worked.
            count2: 1,
        }
    );

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-1");
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

    let executor = executor.restart(Teardown::Drop).await?;

    let result: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?;

    assert_eq!(result, Ok("hello world".to_string()));

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "IfsUpdate",
            &[initial_file(
                "foo.txt",
                "/foo.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .store()
        .await?;

    let agent_id = agent_key!("IfsUpdate", "ifs-update-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_before_update = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_before_update, "foo\n");
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "IfsUpdate",
                "it_initial_file_system_release",
                vec![initial_file(
                    "bar.txt",
                    "/foo.txt",
                    AgentFilePermissions::ReadOnly,
                )],
            )
            .await?;

        executor
            .auto_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_update = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_update, "foo\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_crash, "foo\n");
    }

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_reload, "bar\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_crash, "bar\n");
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "IfsUpdate",
                "it_initial_file_system_release",
                vec![initial_file(
                    "baz.txt",
                    "/foo.txt",
                    AgentFilePermissions::ReadOnly,
                )],
            )
            .await?;

        executor
            .manual_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_manual_update = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_manual_update, "restored");
    }

    executor
        .invoke_and_await_agent(&component, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

        assert_eq!(content_after_reload, "baz\n");
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "get_file_content",
                data_value!(),
            )
            .await?;

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (sender, mut latch) = tokio::sync::mpsc::channel::<()>(1);

    let route = {
        let sender = Arc::new(sender);
        let first_request = Arc::new(tokio::sync::Mutex::new(true));
        Router::new().route(
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
        )
    };
    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "IfsUpdateInsideExportedFunction",
            &[initial_file(
                "foo.txt",
                "/foo.txt",
                AgentFilePermissions::ReadOnly,
            )],
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

    let agent_id = agent_key!("IfsUpdateInsideExportedFunction", "ifs-update-1");
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
                vec![initial_file(
                    "bar.txt",
                    "/foo.txt",
                    AgentFilePermissions::ReadOnly,
                )],
            )
            .await?;

        executor
            .auto_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let result: (String, String) = executor
            .invoke_and_await_typed_with_key(
                &component,
                &agent_id,
                &idempotency_key,
                "run",
                data_value!(),
            )
            .await?;

        assert_eq!(
            result,
            ("foo\n".to_string(), "bar\n".to_string()),
            "the in-flight invocation reads the old file before the update and the new one after"
        );
    }

    drop(http_server);
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Environment", "environment-service-1");
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result: Result<Vec<(String, String)>, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_environment", data_value!())
        .await?;
    let result_p3: Result<Vec<(String, String)>, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_environment_p3", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result_p3, result,
        "the P3-native wasi:cli/environment@0.3 import sees the same environment as the P2/std path"
    );
    assert_eq!(
        result,
        Ok([
            ("TEST_ENV", "test-value".to_string()),
            ("GOLEM_AGENT_ID", worker_id.agent_id.to_string()),
            ("GOLEM_AGENT_KEY", agent_id.to_string()),
            ("GOLEM_COMPONENT_REVISION", "0".to_string()),
            ("GOLEM_AGENT_TYPE", "Environment".to_string()),
        ]
        .map(|(name, value)| (name.to_string(), value))
        .to_vec())
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let call_count = Arc::new(AtomicU8::new(0));

    let route = Router::new().route(
        "/",
        post(move |headers: HeaderMap, body: Bytes| async move {
            let header = headers.get("X-Test").unwrap().to_str().unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();
            let old_count = call_count.fetch_add(1, Ordering::SeqCst);
            match old_count {
                0 => (StatusCode::OK, format!("response is {header} {body}")),
                _ => (StatusCode::NOT_FOUND, "".to_string()),
            }
        }),
    );

    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("HttpClient");
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

    drop(http_server);

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
    let idempotency_keys_clone = idempotency_keys.clone();

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
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    );

    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let _handle = executor.spawn_invoke_and_await_with_key(
        &component,
        &agent_id,
        &key,
        "slow_body_stream",
        data_value!(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?; // Potential "body stream was interrupted" error

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, SHORT_STATUS_TIMEOUT)
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
    drop(http_server);

    assert_eq!(result.into_typed::<u64>()?, 100u64 * 1024u64);

    let idempotency_keys = idempotency_keys.lock().unwrap();
    assert!(
        idempotency_keys.len() == 2 && idempotency_keys[0] == idempotency_keys[1],
        "the retried request repeats the idempotency key of the interrupted one: {idempotency_keys:?}"
    );

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
    let idempotency_keys_clone = idempotency_keys.clone();

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
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    );

    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("HttpClient3");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let _handle = executor.spawn_invoke_and_await_with_key(
        &component,
        &agent_id,
        &key,
        "slow_body_stream",
        data_value!(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?; // Potential "body stream was interrupted" error

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, SHORT_STATUS_TIMEOUT)
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
    drop(http_server);

    assert_eq!(result.into_typed::<u64>()?, 100u64 * 1024u64);

    let idempotency_keys = idempotency_keys.lock().unwrap();
    assert!(
        idempotency_keys.len() == 2 && idempotency_keys[0] == idempotency_keys[1],
        "the retried request repeats the idempotency key of the interrupted one: {idempotency_keys:?}"
    );

    Ok(())
}

#[test]
#[test_r::tag(latency)]
#[tracing::instrument]
async fn sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "clock-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(10u64))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(0u64))
        .await?;
    let duration = start.elapsed();

    assert!(
        duration.as_secs() < 2,
        "a zero-length sleep after recovery returns at once, took {duration:?}"
    );
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(1u64))
        .await?;

    let result = executor
        .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(
        duration.as_secs() >= 1,
        "the sleep lasted, took {duration:?}"
    );
    assert!(result, "the worker is healthy after the sleep");
    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "clock-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let owned_agent_id = worker_id.clone();

    let start = Instant::now();
    let sleep = executor.invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(12u64));
    tokio::pin!(sleep);
    tokio::select! {
        result = &mut sleep => return Err(anyhow!("long sleep returned before unloading: {result:?}")),
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT) => {
            status?;
        }
    }
    tokio::select! {
        result = &mut sleep => return Err(anyhow!("long sleep returned before unloading: {result:?}")),
        unloaded = wait_for_timer_owner_unload(&executor, &owned_agent_id) => { unloaded?; }
    }
    sleep.await?;

    let result = executor
        .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(
        duration.as_secs() >= 12,
        "the sleep lasted, took {duration:?}"
    );
    assert!(
        result,
        "the worker is healthy after resuming from the suspended sleep"
    );

    Ok(())
}

async fn wait_for_timer_owner_unload(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> anyhow::Result<()> {
    within(
        UNLOAD_TIMEOUT,
        "the suspended timer owner to unload",
        async {
            while executor.worker_is_loaded(owner).await {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        },
    )
    .await
}

async fn assert_short_p3_timer_wins_without_suspension(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentKey,
    worker_id: &OwnedAgentId,
    timers: Vec<u64>,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(component, agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(worker_id).await?;
    let invocation =
        executor.invoke_and_await_agent(component, agent_id, "race_p3_sleeps", data_value!(timers));
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => assert_eq!(result?.into_typed::<u64>()?, 2),
        status = executor.wait_for_status(worker_id, AgentStatus::Suspended, SHORT_STATUS_TIMEOUT) => {
            status?;
            return Err(anyhow!("a long sibling timer suspended the owner before the two-second timer completed"));
        }
    }
    assert!(
        executor
            .get_oplog(worker_id, boundary)
            .await?
            .iter()
            .all(|entry| { !matches!(entry.entry, PublicOplogEntry::Suspend(_)) }),
        "short timer race must not suspend, even transiently"
    );
    Ok(())
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_short_timer_blocks_suspension_with_long_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    p3_short_timer_blocks_suspension(last_unique_id, deps, host_api_tests, vec![2, 60]).await
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_short_timer_blocks_suspension_with_multiple_long_watchdogs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    p3_short_timer_blocks_suspension(last_unique_id, deps, host_api_tests, vec![2, 60, 600]).await
}

async fn p3_short_timer_blocks_suspension(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    timers: Vec<u64>,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", format!("p3-short-timer-{timers:?}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    assert_short_p3_timer_wins_without_suspension(
        &executor, &component, &agent_id, &worker_id, timers,
    )
    .await
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_polling_loop_completes_without_watchdog_suspension(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "p3-polling-loop");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let boundary = executor.oplog_max_index(&worker_id).await?;
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "polling_loop_vs_watchdog",
            data_value!(),
        )
        .await?;
    assert_eq!(result, "flag");
    assert!(
        executor
            .get_oplog(&worker_id, boundary)
            .await?
            .iter()
            .all(|entry| { !matches!(entry.entry, PublicOplogEntry::Suspend(_)) }),
        "polling loop with a short timer must not suspend"
    );
    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?
    );
    Ok(())
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_all_long_timers_suspend_and_resume_at_earliest_deadline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "p3-all-long-timers");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let owned_agent_id = worker_id.clone();
    let started = Instant::now();
    let invocation = executor.invoke_and_await_agent(
        &component,
        &agent_id,
        "race_p3_sleeps",
        data_value!(vec![12u64, 60u64]),
    );
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => return Err(anyhow!("all-long timer race returned before suspension: {result:?}")),
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT) => { status?; }
    }
    tokio::select! {
        result = &mut invocation => return Err(anyhow!("all-long timer race returned before unloading: {result:?}")),
        unloaded = wait_for_timer_owner_unload(&executor, &owned_agent_id) => { unloaded?; }
    }
    assert_eq!(invocation.await?.into_typed::<u64>()?, 12);
    assert!(started.elapsed() >= Duration::from_secs(12));
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?
    );
    Ok(())
}

#[test]
#[test_r::tag(latency)]
#[timeout("120s")]
#[tracing::instrument]
async fn p3_repeated_short_timer_races_preserve_shared_scheduler_wakeups(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;

    for index in 0..20 {
        let agent_id = agent_key!("Clock", format!("p3-shared-scheduler-short-{index}"));
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        let timers = if index % 2 == 0 {
            vec![2, 60]
        } else {
            vec![2, 60, 600]
        };
        assert_short_p3_timer_wins_without_suspension(
            &executor, &component, &agent_id, &worker_id, timers,
        )
        .await?;
        assert!(
            executor
                .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
                .await?
        );
    }

    let run_long_race = |index| {
        let executor = &executor;
        let component = &component;
        async move {
            let agent_id = agent_key!("Clock", format!("p3-shared-scheduler-long-{index}"));
            let worker_id = executor
                .start_agent(&component.id, agent_id.clone())
                .await?;
            let owner = worker_id.clone();
            let started = Instant::now();
            let invocation = executor.invoke_and_await_agent(
                component,
                &agent_id,
                "race_p3_sleeps",
                data_value!(vec![12u64, 60u64, 600u64]),
            );
            tokio::pin!(invocation);
            tokio::select! {
                result = &mut invocation => return Err(anyhow!("long timer owner {index} returned before suspension: {result:?}")),
                status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT) => { status?; }
            }
            tokio::select! {
                result = &mut invocation => return Err(anyhow!("long timer owner {index} returned before unloading: {result:?}")),
                unloaded = wait_for_timer_owner_unload(executor, &owner) => { unloaded?; }
            }
            assert_eq!(invocation.await?.into_typed::<u64>()?, 12);
            assert!(started.elapsed() >= Duration::from_secs(12));
            Ok::<(), anyhow::Error>(())
        }
    };
    tokio::try_join!(run_long_race(0), run_long_race(1))?;
    Ok(())
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_short_timer_blocks_suspension_while_promise_is_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "p3-promise-short-timer");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(&worker_id).await?;
    let invocation = executor.invoke_and_await_agent(
        &component,
        &agent_id,
        "race_promise_and_p3_sleep",
        data_value!(2u64),
    );
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => assert_eq!(result?.into_typed::<String>()?, "timer"),
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, SHORT_STATUS_TIMEOUT) => {
            status?;
            return Err(anyhow!("pending promise overrode the short timer and suspended the owner"));
        }
    }
    assert!(
        executor
            .get_oplog(&worker_id, boundary)
            .await?
            .iter()
            .all(|entry| { !matches!(entry.entry, PublicOplogEntry::Suspend(_)) }),
        "promise with short timer must not suspend, even transiently"
    );
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "p3-sleep-suspends-and-resumes");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let mut fiber =
        executor.spawn_invoke_and_await(&component, &agent_id, "sleep_p3", data_value!(25u64));

    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow!("sleep_p3 returned before suspending: {:?}", invoke_result));
        }
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT) => {
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
    // Keep the parked wait from suspending during the test, so the interrupt must be delivered
    // to the *parked* host future rather than to an already-suspended worker.
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::configured(|config| {
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        }),
    )
    .await?;
    let agent_id = agent_key!("Clock", "interrupt-while-parked-in-p3-sleep");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    // Loading the worker does not wait for initialization to finish. Keep its Finished entry
    // before the baseline so it cannot be counted as completion of the interrupted sleep.
    executor
        .wait_for_oplog(
            &worker_id,
            STATUS_TIMEOUT,
            "the initialization to finish",
            |oplog| count_entries(oplog, is_invocation_finished) > 0,
        )
        .await?;
    let before_invocation = executor.oplog_max_index(&worker_id).await?;

    // Action: interrupt the invocation while it is parked in the P3 clock wait.
    let fiber =
        executor.spawn_invoke_and_await(&component, &agent_id, "sleep_p3", data_value!(15u64));
    tokio::time::sleep(PARK_DELAY).await;
    let interrupted_at = Instant::now();
    executor.interrupt(&worker_id).await?;

    // If the interrupt were not delivered to the parked wait, the sleep would run to completion
    // and the invocation would succeed instead.
    let failure = fiber.await?.expect_err("the interrupted invocation fails");
    assert_interrupted(&failure);
    assert!(
        interrupted_at.elapsed() < STATUS_TIMEOUT,
        "interrupting a parked P3 sleep must unwind promptly"
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await?;

    let interrupted_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let incomplete_waits = incomplete_starts(&interrupted_oplog, before_invocation, |name| {
        name.contains("wait")
    });
    assert_eq!(
        incomplete_waits.len(),
        1,
        "the interrupted P3 clock wait must have one incomplete Start"
    );
    let wait_start = incomplete_waits[0];

    // The Resumed hint must immediately make the retained invocation visible as running, while
    // its original durable wait is still incomplete.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, HINT_TIMEOUT)
        .await?;
    let after_first_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        WaitProgress::of(&after_first_resume, wait_start, before_invocation),
        WaitProgress {
            wait_starts: 1,
            wait_ends: 0,
            invocations_started: 1,
            invocations_finished: 0,
        },
        "after the first resume the invocation runs and its wait is still incomplete"
    );
    let first_interrupted = after_first_resume
        .iter()
        .rposition(|entry| matches!(&entry.entry, PublicOplogEntry::Interrupted(_)))
        .expect("missing first Interrupted hint");
    assert!(
        after_first_resume[first_interrupted + 1..]
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::Resumed(_))),
        "resume must commit a Resumed hint after Interrupted"
    );
    let interrupts_after_first_resume = count_entries(&after_first_resume, is_interrupted);

    // A stale Interrupted status used to make this second interrupt a no-op.
    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, HINT_TIMEOUT)
        .await?;
    let after_second_interrupt = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        count_entries(&after_second_interrupt, is_interrupted) > interrupts_after_first_resume,
        "the second interrupt must append another Interrupted hint"
    );

    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, HINT_TIMEOUT)
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, RECOVERY_TIMEOUT)
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        WaitProgress::of(&oplog, wait_start, before_invocation),
        WaitProgress {
            wait_starts: 1,
            wait_ends: 1,
            invocations_started: 1,
            invocations_finished: 1,
        },
        "restart must complete the original wait and invocation exactly once"
    );

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(0u64))
        .await?;
    assert!(
        start.elapsed().as_secs() < 2,
        "a zero-length P3 sleep after recovery returns at once"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    Ok(())
}

/// Interrupting a worker while the guest is parked in a live P3 TCP `receive` wait (connected,
/// but the peer never sends any bytes) must deliver the interrupt promptly: the parked durable
/// receive task races the worker's interrupt signal, abandons its open chunk child and parent
/// durable calls (both `Start`s stay incomplete for replay) and unwinds the event loop
/// cooperatively, closing the socket. After resume, the retained invocation reports connection
/// loss; only a new guest call opens another connection.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn interrupt_while_parked_in_p3_tcp_receive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let mut server = SilentFirstTcpServer::start().await?;
    let port = server.port;
    let agent_id = agent_key!("Networking", "interrupt-while-parked-in-p3-tcp-receive");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let before_invocation = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .map(|entry| entry.oplog_index)
        .unwrap_or(OplogIndex::INITIAL);

    let key = IdempotencyKey::fresh();
    let stored_key = executor
        .external_invocation_key(&worker_id.agent_id, &key, "tcp_collect_p3")
        .await?;
    let method_progress = |oplog: &[PublicOplogEntryWithIndex]| {
        method_invocation_progress(oplog, before_invocation, &stored_key, "tcp_collect_p3")
    };
    let receive_starts = |oplog: &[PublicOplogEntryWithIndex]| {
        [TCP_RECEIVE, TCP_RECEIVE_CHUNK].map(|function_name| {
            oplog
                .starts_of(function_name)
                .into_iter()
                .map(|entry| entry.oplog_index)
                .filter(|index| *index > before_invocation)
                .collect::<Vec<_>>()
        })
    };

    // Phase 1: park the invocation in the TCP receive.
    let fiber = executor.spawn_invoke_and_await_with_key(
        &component,
        &agent_id,
        &key,
        "tcp_collect_p3",
        data_value!(port),
    );
    // The accepted connection alone does not prove that the guest reached the receive wait.
    // Wait for the receive parent and chunk Starts, and require both to remain open while the
    // retained invocation has no Finished entry.
    within(
        CONNECT_TIMEOUT,
        "the initial P3 TCP connection",
        server.connected.recv(),
    )
    .await?
    .ok_or_else(|| anyhow!("tcp server stopped before the guest connected"))?;
    let parked = executor
        .wait_for_committed_oplog(
            &worker_id,
            CONNECT_TIMEOUT,
            "open receive parent and chunk Starts with one unfinished invocation",
            |oplog| {
                let [receive, chunk] = receive_starts(oplog);
                receive.len() == 1
                    && chunk.len() == 1
                    && oplog.call(receive[0]).terminal_count() == 0
                    && oplog.call(chunk[0]).terminal_count() == 0
                    && method_progress(oplog) == (1, 0)
            },
        )
        .await?;
    let [receive, chunk] = receive_starts(&parked);
    let interrupted_starts = [(TCP_RECEIVE, receive[0]), (TCP_RECEIVE_CHUNK, chunk[0])];
    assert_eq!(
        server.accepted.load(Ordering::SeqCst),
        1,
        "the parked invocation opened one connection"
    );
    let committed_prefix = executor.stored_oplog(&worker_id.agent_id).await;

    // Phase 2: interrupt the parked invocation.
    executor
        .interrupt(&worker_id)
        .await
        .context("P3 TCP interrupt request")?;
    let failure = fiber
        .await
        .context("P3 TCP interrupted invocation join")?
        .expect_err("the interrupted invocation fails");
    assert_interrupted(&failure);
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await
        .context("P3 TCP Interrupted status after stop")?;
    assert!(
        within(
            STATUS_TIMEOUT,
            "the TCP socket to close after the interrupt",
            server.closed.recv()
        )
        .await?
        .unwrap_or(false),
        "interrupting the worker must close the in-flight TCP connection"
    );
    let before_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for (function_name, interrupted_start) in interrupted_starts {
        assert_eq!(
            before_resume.call(interrupted_start).terminal_count(),
            0,
            "the captured {function_name} Start must have no End or Cancelled terminal before resume"
        );
    }
    assert_eq!(
        method_progress(&before_resume).1,
        0,
        "the interrupted retained invocation must remain unfinished before resume"
    );

    // Phase 3: the original call settles with typed loss, without another peer connection or
    // new Start.
    executor
        .resume(&worker_id, false)
        .await
        .context("P3 TCP resume request")?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, LONG_STATUS_TIMEOUT)
        .await
        .context("P3 TCP retained invocation reaches Idle after resume")?;
    let after_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let raw_after_resume = executor.stored_oplog(&worker_id.agent_id).await;
    assert!(
        raw_after_resume.len() >= committed_prefix.len()
            && raw_after_resume[..committed_prefix.len()] == committed_prefix[..],
        "resume keeps the committed oplog prefix unchanged"
    );
    assert!(
        !after_resume
            .iter()
            .any(|entry| entry.oplog_index > before_invocation
                && matches!(entry.entry, PublicOplogEntry::Jump(_))),
        "resume does not jump back in the oplog"
    );
    assert_eq!(
        method_progress(&after_resume),
        (1, 1),
        "resume must finish the retained invocation exactly once before the fresh call"
    );
    let recovered = executor
        .invoke_and_await_typed_with_key::<Result<String, String>>(
            &component,
            &agent_id,
            &key,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?;
    assert_eq!(recovered, Err("ErrorCode::ConnectionBroken".to_string()));
    let [receive, chunk] = receive_starts(&after_resume);
    for ((function_name, interrupted_start), starts) in
        interrupted_starts.into_iter().zip([receive, chunk])
    {
        let call = after_resume.call(interrupted_start);
        assert_eq!(
            (starts, call.ends.len(), call.cancellations.len()),
            (vec![interrupted_start], 1, 0),
            "{function_name} keeps its original Start and has one End under it"
        );
    }
    assert_eq!(
        server.accepted.load(Ordering::SeqCst),
        1,
        "replay of the completed connect must not open another external connection"
    );

    // Phase 4: fresh invocations connect again, and replay does not.
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "tcp_collect_p3", data_value!(port))
        .await
        .context("P3 TCP fresh liveness invocation")?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(result2, Ok("hello".to_string()));
    assert_eq!(
        server.accepted.load(Ordering::SeqCst),
        2,
        "only the fresh liveness invocation may open the second connection"
    );

    executor.simulated_crash(&worker_id).await?;
    let result3 = executor
        .invoke_and_await_typed::<Result<String, String>>(
            &component,
            &agent_id,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?;
    assert_eq!(result3, Ok("hello".to_string()));
    assert_eq!(
        server.accepted.load(Ordering::SeqCst),
        3,
        "replaying completed receive results must not open extra connections"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    server.task.abort();

    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn incomplete_p3_udp_receive_repairs_original_start_and_replays_completed_datagram(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let peer = UdpSocket::bind("127.0.0.1:0").await?;
    let ready_port = peer.local_addr()?.port();
    let reservation = UdpSocket::bind("127.0.0.1:0").await?;
    let address = reservation.local_addr()?;
    let port = address.port();
    drop(reservation);
    let agent_id = agent_key!("Networking", "incomplete-p3-udp-receive");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let key = IdempotencyKey::fresh();
    let mut buffer = [0; 32];
    let receive_ready = async |buffer: &mut [u8]| -> anyhow::Result<()> {
        let (len, source) = within(
            CONNECT_TIMEOUT,
            "the guest's readiness datagram",
            peer.recv_from(buffer),
        )
        .await??;
        assert_eq!(
            (&buffer[..len], source),
            (&b"ready"[..], address),
            "the guest announces from its bound address that it is receiving"
        );
        Ok(())
    };

    // Phase 1: interrupt the receive while no datagram arrived.
    let invocation = executor.spawn_invoke_and_await_with_key(
        &component,
        &agent_id,
        &key,
        "udp_receive_p3",
        data_value!(port, ready_port),
    );
    receive_ready(&mut buffer).await?;
    tokio::time::sleep(PARK_DELAY).await;
    executor.interrupt(&worker_id).await?;
    let interrupted = invocation
        .await?
        .expect_err("pending receive must be interrupted");
    assert_interrupted(&interrupted);
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let receive_start = oplog
        .starts_of(UDP_RECEIVE)
        .first()
        .expect("receive must retain its original Start")
        .oplog_index;
    assert!(
        oplog.call(receive_start).ends.is_empty(),
        "the interrupted receive has no End"
    );

    // Phase 2: reconstruction rebinds the socket and repairs the original receive with the next
    // datagram.
    executor.resume(&worker_id, false).await?;
    let recovered = within(CONNECT_TIMEOUT, "the repaired receive", async {
        let result = executor.invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "udp_receive_p3",
            data_value!(port, ready_port),
        );
        tokio::pin!(result);
        let mut interval = tokio::time::interval(DATAGRAM_INTERVAL);
        loop {
            tokio::select! {
                result = &mut result => break result,
                _ = interval.tick() => { peer.send_to(b"later", address).await?; }
            }
        }
    })
    .await??
    .into_typed::<Result<String, String>>()?;
    assert_eq!(recovered, Ok("later".to_string()));
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        (
            oplog.call(receive_start).ends.len(),
            oplog.starts_of(UDP_RECEIVE).len()
        ),
        (1, 1),
        "the repair ends the original receive Start and starts no other receive"
    );

    // Phase 3: fresh receives work, before and after a crash that replays the repaired one.
    for payload in ["fresh", "after-restart"] {
        if payload == "after-restart" {
            executor.simulated_crash(&worker_id).await?;
            let replayed = executor
                .invoke_and_await_typed_with_key::<Result<String, String>>(
                    &component,
                    &agent_id,
                    &key,
                    "udp_receive_p3",
                    data_value!(port, ready_port),
                )
                .await?;
            assert_eq!(replayed, Ok("later".to_string()));
        }
        let (result, sent) = tokio::join!(
            executor.invoke_and_await_agent(
                &component,
                &agent_id,
                "udp_receive_p3",
                data_value!(port, ready_port)
            ),
            async {
                receive_ready(&mut buffer).await?;
                peer.send_to(payload.as_bytes(), address).await?;
                Ok::<_, anyhow::Error>(())
            }
        );
        sent?;
        assert_eq!(
            result?.into_typed::<Result<String, String>>()?,
            Ok(payload.to_string())
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        (
            oplog.starts_of(UDP_RECEIVE).len(),
            oplog.starts_of(UDP_SEND).len(),
            oplog.call(receive_start).ends.len(),
        ),
        (3, 3, 1),
        "two fresh receives add their own calls; the repaired receive keeps its one End"
    );
    assert!(
        tokio::time::timeout(SILENCE_WINDOW, peer.recv_from(&mut buffer))
            .await
            .is_err(),
        "completed replay must not send another readiness datagram"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn synchronous_clock_overlaps_http_and_replays_offline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let release_response = Arc::new(Semaphore::new(0));
    let response_gate = release_response.clone();
    let route = Router::new().route(
        "/simulated-slow-request",
        get(move || {
            let response_gate = response_gate.clone();
            async move {
                response_gate.acquire().await.unwrap().forget();
                "response after clock completed"
            }
        }),
    );
    let mut server = JoinSet::new();
    server.spawn(async move { axum::serve(listener, route).await });

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_key!("Clock", "synchronous-clock-concurrent-http");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    release_response.add_permits(1);
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "p2_clock_during_request",
            data_value!(),
        )
        .await?;
    assert_eq!(result, "response after clock completed");

    // Completed replay must reuse both the clock value and the HTTP response with the peer gone.
    let before_replay = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    server.abort_all();
    while server.join_next().await.is_some() {}
    executor.simulated_crash(&worker_id).await?;
    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?
    );
    let after_replay = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let count_calls = |entries: &[PublicOplogEntryWithIndex]| {
        entries
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(start)
                        if start.function_name == "wall_clock::now"
                            || start.function_name == "http::client::send"
                )
            })
            .count()
    };
    assert_eq!(
        count_calls(&after_replay),
        count_calls(&before_replay),
        "replay reuses the recorded clock and HTTP calls and starts no new ones"
    );
    Ok(())
}

/// A Clock agent of host-api-tests whose `PORT` points to a local HTTP server.
struct ClockWithServer {
    context: TestContext,
    executor: TestWorkerExecutor,
    component: ComponentDto,
    server: LocalHttpServer,
    agent_id: ParsedAgentKey,
    worker_id: OwnedAgentId,
}

impl ClockWithServer {
    async fn start(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        host_api_tests: &PrecompiledComponent,
        server: LocalHttpServer,
        agent_name: &str,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .with_env(
                "Clock",
                vec![("PORT".to_string(), server.port().to_string())],
            )
            .store()
            .await?;
        let agent_id = agent_key!("Clock", agent_name);
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        Ok(Self {
            context,
            executor,
            component,
            server,
            agent_id,
            worker_id,
        })
    }
}

async fn simulated_slow_request_server(delay: Duration) -> LocalHttpServer {
    counting_slow_request_server(delay).await.server
}

/// A local HTTP server and the number of requests it received.
struct CountingServer {
    server: LocalHttpServer,
    requests: Arc<AtomicUsize>,
}

/// Like [`simulated_slow_request_server`], but also counts the requests the server receives.
async fn counting_slow_request_server(delay: Duration) -> CountingServer {
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = requests.clone();
    let route = Router::new().route(
        "/simulated-slow-request",
        get(move || async move {
            server_requests.fetch_add(1, Ordering::AcqRel);
            tokio::time::sleep(delay).await;
            "slow response".to_string()
        }),
    );
    let server = LocalHttpServer::serve(route).await.unwrap();
    CountingServer { server, requests }
}

/// Creates an HTTP server with a streaming endpoint that sends many small chunks
/// with delays between them, mimicking OpenAI-style SSE streaming.
/// Each chunk is a small piece of text sent with a configurable delay.
async fn streaming_chunk_server(chunk_count: usize, chunk_delay: Duration) -> LocalHttpServer {
    let route = Router::new()
        .route(
            "/streaming-chunks",
            get(move || async move {
                let stream = stream::iter(0..chunk_count)
                    .throttle(chunk_delay)
                    .map(move |i| Ok::<Bytes, BoxError>(Bytes::from(format!("chunk-{i}\n"))));

                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/plain")
                    .header("Transfer-Encoding", "chunked")
                    .body(Body::from_stream(stream))
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

    LocalHttpServer::serve(route).await.unwrap()
}

#[test]
#[tracing::instrument]
async fn sleep_less_than_suspend_threshold_while_awaiting_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ClockWithServer {
        executor,
        component,
        agent_id,
        worker_id,
        server,
        ..
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(10)).await,
        "clock-service-4",
    )
    .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(2u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(
        (2..10).contains(&duration.as_secs()),
        "expected 2..10 s, took {duration:?}"
    );
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
    let ClockWithServer {
        executor,
        component,
        agent_id,
        worker_id,
        server,
        ..
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(5)).await,
        "clock-service-5",
    )
    .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(30u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(
        (5..30).contains(&duration.as_secs()),
        "expected 5..30 s, took {duration:?}"
    );
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
    let ClockWithServer {
        executor,
        component,
        agent_id,
        worker_id,
        server,
        ..
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(30)).await,
        "clock-service-6",
    )
    .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_request",
            data_value!(15u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(
        (15..30).contains(&duration.as_secs()),
        "expected 15..30 s, took {duration:?}"
    );
    assert_eq!(result, "Timeout");

    Ok(())
}

/// Mixed-ABI suspend regression: one guest task awaits a slow P3 `wasi:http` send while another
/// blocks in a P2 sleep (`thread::sleep` → `wasi:io/poll`) longer than the suspend threshold.
/// The worker must not be suspended while the P3 request is in flight — a premature suspend
/// would drop the pending host call and re-execute the HTTP request on resume, so the server
/// receiving the request exactly once proves the P3 completion was delivered.
#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let CountingServer {
        server,
        requests: request_count,
    } = counting_slow_request_server(Duration::from_secs(5)).await;
    let ClockWithServer {
        executor,
        component,
        agent_id,
        worker_id,
        server,
        ..
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        server,
        "clock-service-p2-sleep-during-request",
    )
    .await?;
    let boundary = executor.oplog_max_index(&worker_id).await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "p2_sleep_during_request",
            data_value!(15u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?,
        "the worker must accept a follow-up invocation after the mixed P2/P3 call"
    );

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

    let suspend_count = executor
        .get_oplog(&worker_id, boundary)
        .await?
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_)))
        .count();
    assert!(
        suspend_count <= 1,
        "the completed request may leave the P2 timer as the sole suspendable operation, but the \
         mixed invocation must not repeatedly suspend"
    );

    drop(server);
    drop(executor);

    Ok(())
}

/// Opposite mixed-ABI completion order: the P2 sleep completes first while the P3 HTTP request
/// remains pending. Neither operation may be replayed, and a pending request must prevent a
/// suspension after the P2 timer completes.
#[test]
#[timeout("35s")]
#[tracing::instrument]
async fn p2_sleep_completes_while_p3_request_remains_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let CountingServer {
        server,
        requests: request_count,
    } = counting_slow_request_server(Duration::from_secs(16)).await;
    let ClockWithServer {
        executor,
        component,
        agent_id,
        worker_id,
        server,
        ..
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        server,
        "clock-service-p2-first-completion",
    )
    .await?;
    let boundary = executor.oplog_max_index(&worker_id).await?;

    let started = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "p2_sleep_during_request",
            data_value!(12u64),
        )
        .await?;

    assert_eq!(result, "slow response, slept");
    assert_eq!(request_count.load(Ordering::Acquire), 1);
    assert!(started.elapsed() >= Duration::from_secs(16));
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(
        executor
            .get_oplog(&worker_id, boundary)
            .await?
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Suspend(_))),
        "the executor must not suspend while the HTTP request remains pending"
    );
    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?
    );

    drop(server);
    Ok(())
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn p2_poll_preserves_duplicate_indices_and_repeated_readiness(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "clock-service-p2-poll-duplicates");

    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "p2_poll_duplicate_handles",
            data_value!(12_000u64, 100u64),
        )
        .await?;

    assert_eq!(result, "[1, 2];[0, 1];[0];[0];[0]");
    assert!(
        executor
            .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
            .await?
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p2_file_pollables_ready_and_block_survive_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::configured(full_replay_config),
    )
    .await?;
    let agent_id = agent_key!("Clock", "p2-file-pollables-replay");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for (contents, expected) in [("abcdefg", "abcdefg"), ("HIJKL", "abcdefgHIJKL")] {
        let result = executor
            .invoke_and_await_typed::<String>(
                &component,
                &agent_id,
                "p2_file_pollables",
                data_value!(contents.to_string()),
            )
            .await?;
        assert_eq!(result, format!("true;true;{expected}"));
        executor.simulated_crash(&worker_id).await?;
        assert!(
            executor
                .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
                .await?
        );
    }
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
    let ClockWithServer {
        context,
        executor,
        component,
        agent_id,
        worker_id,
        server,
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(2)).await,
        "clock-service-7",
    )
    .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(20u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(server);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
        .await?;

    assert!(
        (10..20).contains(&duration.as_secs()),
        "expected 10..20 s, took {duration:?}"
    );
    assert_eq!(
        result,
        "Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n"
    );
    assert!(healthcheck_result);

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
    let ClockWithServer {
        context,
        executor,
        component,
        agent_id,
        worker_id,
        server,
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(1)).await,
        "clock-service-8",
    )
    .await?;

    executor.log_output(&worker_id).await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 5u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
        .await?;

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
    let ClockWithServer {
        context,
        executor,
        component,
        agent_id,
        worker_id,
        server,
    } = ClockWithServer::start(
        deps,
        last_unique_id,
        host_api_tests,
        simulated_slow_request_server(Duration::from_secs(1)).await,
        "clock-service-9",
    )
    .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(12u64, 2u64),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_typed::<bool>(&component, &agent_id, "healthcheck", data_value!())
        .await?;

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
    let ExecutorTest {
        context,
        executor,
        component,
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let fiber = executor.spawn_invoke_and_await(&component, &agent_id, "sleep", data_value!(10u64));

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "p3-resuming-sleep");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_agent(&component, &agent_id, "sleep_p3", data_value!(25u64))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT)
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    info!("Restarting worker...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Worker restarted");

    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, SUSPEND_TIMEOUT)
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, LONG_STATUS_TIMEOUT)
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(0u64))
        .await?;
    assert!(start.elapsed().as_secs() < 2);

    Ok(())
}

#[test]
#[timeout("60s")]
async fn p3_ready_timers_defer_delivery_before_direct_clock_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "ready-timers-before-direct-clock");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(&worker).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "pending_p3_timers_and_p2_clock",
            data_value!(),
        )
        .await?;
    assert_eq!(result.into_typed::<Vec<bool>>()?, vec![true, true]);
    let before = executor.get_oplog(&worker, boundary.next()).await?;
    let clocks: Vec<_> = before
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name.ends_with("wall_clock::now") => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    let delivered: Vec<_> = before
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::CompletionDelivered(_) => Some(entry.oplog_index),
            _ => None,
        })
        .collect();
    assert!(
        clocks.len() == 2
            && delivered.len() == 2
            && clocks[0] < delivered[0]
            && clocks[1] < delivered[1],
        "each direct clock read comes before the delivery of a ready timer: clocks {clocks:?}, deliveries {delivered:?}"
    );
    assert_internal_clock_calls_settled(&before);

    let executor = executor.restart(Teardown::Drop).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let after = executor.get_oplog(&worker, boundary.next()).await?;
    assert_eq!(
        after
            .iter()
            .filter(
                |entry| matches!(&entry.entry, PublicOplogEntry::Start(start)
            if start.function_name.ends_with("wall_clock::now"))
            )
            .count(),
        2,
        "replay must reuse both direct clock reads"
    );
    assert_internal_clock_calls_settled(&after);
    Ok(())
}

#[test]
#[timeout("60s")]
async fn p3_wait_for_restart_preserves_recorded_deadline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides::configured(|config| {
        config.suspend.wait_suspend_grace = Duration::from_secs(300);
    });
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_key!("Clock", "recorded-p3-deadline");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(&worker).await?;
    let mut read = executor.gate_next_replay_access_admission(
        &worker.agent_id,
        "monotonic-clock::now",
        ReplayAdmissionStage::AfterHostInternalTerminal,
    );
    executor
        .invoke_agent(&component, &agent_id, "sleep_p3", data_value!(8u64))
        .await?;
    within(
        STATUS_TIMEOUT,
        "the wait's internal clock read to end",
        read.entered(),
    )
    .await?;
    executor.commit_oplog(&worker.agent_id).await?;
    let before = executor.get_oplog(&worker, boundary.next()).await?;
    let clock = internal_clock_read(&before).expect("completed internal read");
    assert!(
        !before.call(clock.read).ends.is_empty() && before.call(clock.wait).ends.is_empty(),
        "the internal clock read has ended, and its wait has not"
    );
    drop(executor);
    drop(read);

    // Let the original deadline expire while there is no executor. Starting a fresh eight-second
    // timer during replay would violate the recorded deadline and exceed the recovery bound.
    tokio::time::sleep(Duration::from_secs(9)).await;
    let executor = start_with_overrides(deps, &context, overrides).await?;
    within(
        RECORDED_DEADLINE_RECOVERY,
        "recovery within the recorded deadline",
        executor.invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!()),
    )
    .await??;
    let after = executor.get_oplog(&worker, boundary.next()).await?;
    assert_eq!(
        after
            .iter()
            .filter(
                |entry| matches!(&entry.entry, PublicOplogEntry::Start(start)
                if start.function_name.ends_with("monotonic-clock::now"))
            )
            .count(),
        1,
        "recovery must reuse the recorded clock read, not append a fresh one"
    );
    assert_eq!(
        after.call(clock.wait).ends.len(),
        1,
        "recovery ends the original wait once"
    );
    assert_internal_clock_calls_settled(&after);
    assert_eq!(
        count_entries(&after, is_invocation_finished),
        2,
        "the sleep and the health check after the boundary both finish"
    );
    Ok(())
}

#[test]
#[timeout("60s")]
async fn p3_wait_for_cancel_after_internal_end_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("Clock", "cancel-after-internal-clock-end");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(&worker).await?;
    // Each wait starts in its own host task, so the waits reach their gates in any order. The
    // test lets one wait at a time pass its `Start` and tells the waits apart by their requests.
    let mut first_start = executor.gate_next_replay_access_admission(
        &worker.agent_id,
        "monotonic-clock::wait-for",
        ReplayAdmissionStage::BeforeLiveStart,
    );
    let mut second_start = executor.gate_next_replay_access_admission(
        &worker.agent_id,
        "monotonic-clock::wait-for",
        ReplayAdmissionStage::BeforeLiveStart,
    );
    let mut first_read = executor.gate_next_replay_access_admission(
        &worker.agent_id,
        "monotonic-clock::now",
        ReplayAdmissionStage::BeforeDeferredStart,
    );
    let mut second_read = executor.gate_next_replay_access_admission(
        &worker.agent_id,
        "monotonic-clock::now",
        ReplayAdmissionStage::BeforeDeferredStart,
    );
    let long_wait: HostRequest = HostRequestMonotonicClockDuration {
        duration_in_nanos: 60_000_000_000,
    }
    .into();
    let long_wait = long_wait.into_typed_schema_value()?;
    let invocation = executor.invoke_and_await_agent(
        &component,
        &agent_id,
        "race_p3_sleeps",
        data_value!(vec![60u64, 0u64]),
    );
    let coordinate = async {
        tokio::join!(first_start.entered(), second_start.entered());
        first_start.release();
        first_read.entered().await;
        executor.commit_oplog(&worker.agent_id).await?;
        let first_is_long = executor
            .get_oplog(&worker, boundary.next())
            .await?
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name.ends_with("monotonic-clock::wait-for") =>
                {
                    Some(start.request.as_ref() == Some(&long_wait))
                }
                _ => None,
            })
            .expect("the first wait's Start");
        second_start.release();
        second_read.entered().await;
        let (long_read, short_read) = if first_is_long {
            (first_read, second_read)
        } else {
            (second_read, first_read)
        };
        // Hold the long wait after the End of its clock read, so the short wait cancels it there.
        let mut read = executor.gate_next_replay_access_admission(
            &worker.agent_id,
            "monotonic-clock::now",
            ReplayAdmissionStage::AfterHostInternalTerminal,
        );
        long_read.release();
        read.entered().await;
        executor.commit_oplog(&worker.agent_id).await?;
        let entries = executor.get_oplog(&worker, boundary.next()).await?;
        let clock = internal_clock_read(&entries).expect("internal read");
        assert!(
            !entries.call(clock.read).ends.is_empty(),
            "the long wait's internal clock read has ended"
        );
        short_read.release();
        anyhow::Ok((clock, read))
    };
    let (result, clock) = within(
        STATUS_TIMEOUT,
        "the timer race and its coordination",
        async { tokio::join!(invocation, coordinate) },
    )
    .await?;
    let (clock, read) = clock?;
    assert_eq!(
        result?.into_typed::<u64>()?,
        0,
        "the zero-second timer wins the race"
    );
    let entries = executor.get_oplog(&worker, boundary.next()).await?;
    assert!(
        !entries.call(clock.wait).cancellations.is_empty(),
        "the short wait cancels the long wait after its clock read ended"
    );
    assert_internal_clock_calls_settled(&entries);
    assert_eq!(
        count_entries(&entries, is_invocation_finished),
        1,
        "only the race invocation finishes after the boundary"
    );
    drop(read);

    let executor = executor.restart(Teardown::Drop).await?;
    within(
        STATUS_TIMEOUT,
        "replay of the cancelled wait",
        executor.invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!()),
    )
    .await??;
    assert_internal_clock_calls_settled(&executor.get_oplog(&worker, boundary.next()).await?);
    Ok(())
}

/// A `monotonic-clock::now` call that a P3 wait made, and that wait.
struct InternalClockRead {
    read: OplogIndex,
    wait: OplogIndex,
}

/// The first `monotonic-clock::now` call in `oplog`; it must belong to a wait.
fn internal_clock_read(oplog: &[PublicOplogEntryWithIndex]) -> Option<InternalClockRead> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(start) if start.function_name.ends_with("monotonic-clock::now") => {
            Some(InternalClockRead {
                read: entry.oplog_index,
                wait: start.parent_start_index.expect("wait owns clock read"),
            })
        }
        _ => None,
    })
}

fn assert_internal_clock_calls_settled(entries: &[PublicOplogEntryWithIndex]) {
    for entry in entries {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            assert!(
                entries.iter().any(|terminal| match &terminal.entry {
                    PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                    PublicOplogEntry::Cancelled(cancelled) =>
                        cancelled.start_index == entry.oplog_index,
                    _ => false,
                }),
                "unsettled Start at {}",
                entry.oplog_index
            );
            if start.function_name.ends_with("monotonic-clock::now")
                && start.parent_start_index.is_some()
            {
                assert!(
                    entries.iter().all(|terminal| match &terminal.entry {
                        PublicOplogEntry::CompletionDelivered(delivered) =>
                            delivered.start_index != entry.oplog_index,
                        PublicOplogEntry::CompletionDiscarded(discarded) =>
                            discarded.start_index != entry.oplog_index,
                        _ => true,
                    }),
                    "internal read must not have a guest delivery marker"
                );
            }
        }
    }
}

#[test]
#[tracing::instrument]
async fn failing_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, agent_counters).await?;
    let agent_id = agent_key!("FailingCounter", "failing-worker-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let failure = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await
        .expect_err("adding a too large value fails");
    let failure_after = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await
        .expect_err("the failed worker rejects the next invocation");

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_worker_error!(
        failure,
        WorkerExecutorError::InvocationFailed { stderr, .. }
            if stderr.contains("error log message") && stderr.contains("value is too large"),
        "the failure carries the guest's stderr, with its log line and panic message"
    );
    assert_worker_error!(
        failure_after,
        WorkerExecutorError::PreviousInvocationFailed { stderr, .. }
            if stderr.contains("error log message") && stderr.contains("value is too large"),
        "the next invocation reports the earlier failure with its stderr and panic message"
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-2");
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

    let executor = executor.restart(Teardown::Drop).await?;

    let result: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?;

    assert_eq!(result, Ok("hello world".to_string()));

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-3");
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

    let times1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-4");
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

    let times1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/"))
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-5");
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

    let result: Result<String, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "read_file", data_value!("/link.txt"))
        .await?;

    assert_eq!(result, Ok("hello world".to_string()));

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-6");
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

    let times_file_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

    let times_dir_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times_dir_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    let times_file_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-7");
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

    let times1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn filesystem_escaping_relative_symlink_is_a_guest_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;

    for (case, target, link) in [
        ("root", "../etc", "/esc"),
        ("nested", "../../etc", "/d/esc"),
    ] {
        let agent_id = agent_key!("FileSystem", format!("escaping-symlink-{case}"));
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;

        let written = executor
            .invoke_and_await_typed::<Result<(), String>>(
                &component,
                &agent_id,
                "write_file",
                data_value!("/control", "sandbox control"),
            )
            .await?;
        assert_eq!(written, Ok(()));

        let inside_link = executor
            .invoke_and_await_typed::<Result<(), String>>(
                &component,
                &agent_id,
                "create_sym_link",
                data_value!("control", "/inside"),
            )
            .await?;
        assert_eq!(inside_link, Ok(()));

        let inside_read = executor
            .invoke_and_await_typed::<Result<String, String>>(
                &component,
                &agent_id,
                "read_file",
                data_value!("/inside"),
            )
            .await?;
        assert_eq!(inside_read, Ok("sandbox control".to_string()));

        let directory = executor
            .invoke_and_await_typed::<Result<(), String>>(
                &component,
                &agent_id,
                "create_directory",
                data_value!("/d"),
            )
            .await?;
        assert_eq!(directory, Ok(()));

        let escaping_link = executor
            .invoke_and_await_typed::<Result<(), String>>(
                &component,
                &agent_id,
                "create_sym_link",
                data_value!(target, link),
            )
            .await?;
        assert_eq!(escaping_link, Ok(()), "creating {link} -> {target}");

        let denied = executor
            .invoke_and_await_typed::<Result<String, String>>(
                &component,
                &agent_id,
                "read_file",
                data_value!(format!("{link}/hosts")),
            )
            .await?;
        assert_eq!(
            denied,
            Err("Permission denied (os error 2)".to_string()),
            "reading through {link} -> {target} must return guest EACCES"
        );

        let promise = executor
            .invoke_and_await_typed::<PromiseId>(
                &component,
                &agent_id,
                "create_release_promise",
                data_value!(),
            )
            .await?;
        assert_eq!(promise.agent_id, worker_id.agent_id);

        let ordinary_read = executor
            .invoke_and_await_typed::<Result<String, String>>(
                &component,
                &agent_id,
                "read_file",
                data_value!("/control"),
            )
            .await?;
        assert_eq!(ordinary_read, Ok("sandbox control".to_string()));

        let metadata = executor
            .wait_for_status(&worker_id, AgentStatus::Idle, SHORT_STATUS_TIMEOUT)
            .await?;
        assert_eq!(metadata.status, AgentStatus::Idle);
    }

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-8");
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

    let times_file_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

    let times_dir_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times_dir_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    let times_file_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-9");
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

    let times_srcdir_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

    let times_destdir_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    let times_file_1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times_srcdir_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

    let times_destdir_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test2"))
        .await?;

    let times_file_2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-10");
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

    let times1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get_info", data_value!("/test"))
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_zeroes_to_file_via_stream",
            data_value!("/testfile.txt", 131_072_u64),
        )
        .await?;

    let times1: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let times2: Result<FileTimestamps, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?;

    let replayed_contents = executor
        .get_file_contents(&worker_id, "/testfile.txt")
        .await?;
    assert_eq!(replayed_contents.len(), 131_072);
    assert!(replayed_contents.iter().all(|byte| *byte == 0));

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("FileSystem", "file-service-3");
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

    let hash1: Result<HashResult, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "hash", data_value!("testfile.txt"))
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    let hash2: Result<HashResult, String> = executor
        .invoke_and_await_typed(&component, &agent_id, "hash", data_value!("testfile.txt"))
        .await?;

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let agent_id = agent_key!("Networking", "ip-address-resolve-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get", data_value!())
        .await?;

    // If the recovery succeeds, the replayed IP address resolution produced the same result.
    let executor = executor.restart(Teardown::Drop).await?;

    let result2: Vec<String> = executor
        .invoke_and_await_typed(&component, &agent_id, "get", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the
    // same order) but we can expect that it could resolve golem.cloud to at least one address.
    assert!(!result1.is_empty(), "the first resolution found an address");
    assert!(
        !result2.is_empty(),
        "the resolution after restart found an address"
    );

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let agent_id = agent_key!("Networking", "p3-ip-address-resolve-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1: Result<Vec<String>, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("golem.cloud"),
        )
        .await?;

    let executor = executor.restart(Teardown::Drop).await?;

    // If the recovery succeeds, the replayed P3 DNS resolution produced the same recorded result

    let result2: Result<Vec<String>, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("golem.cloud"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the
    // same order) but we can expect that it could resolve golem.cloud to at least one address.
    let entries1 = result1.expect("the first resolution succeeds");
    let entries2 = result2.expect("the resolution after restart succeeds");
    assert!(
        !entries1.is_empty(),
        "the first resolution found an address"
    );
    assert!(
        !entries2.is_empty(),
        "the resolution after restart found an address"
    );

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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start_with_overrides(
        deps,
        last_unique_id,
        host_api_tests,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let agent_id = agent_key!("Networking", "p3-ip-address-resolve-failure-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result: Result<Vec<String>, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "resolve_p3",
            data_value!("this-name-does-not-exist.golem-test.invalid"),
        )
        .await?;

    let error = result.expect_err("the resolution error is visible to the guest");
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
    let ExecutorTest {
        executor,
        component,
        ..
    } = ExecutorTest::start(deps, last_unique_id, host_api_tests).await?;
    let agent_id = agent_key!("WasiConfig", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            vec![config_entry("k1", "v1"), config_entry("k2", "v2")],
        )
        .await?;

    {
        // get existing key

        let result: Option<String> = executor
            .invoke_and_await_typed(&component, &agent_id, "get", data_value!("k1"))
            .await?;

        assert_eq!(result, Some("v1".to_string()))
    }

    {
        // get non-existent key

        let result: Option<String> = executor
            .invoke_and_await_typed(&component, &agent_id, "get", data_value!("k3"))
            .await?;

        assert_eq!(result, None)
    }

    {
        // get all keys

        let mut result: Vec<(String, String)> = executor
            .invoke_and_await_typed(&component, &agent_id, "get_all", data_value!())
            .await?;
        result.sort();

        assert_eq!(result, string_pairs(&[("k1", "v1"), ("k2", "v2")]))
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_agent_config(
            "WasiConfig",
            vec![config_entry("k1", "v0"), config_entry("k3", "v3")],
        )
        .store()
        .await?;

    let agent_id = agent_key!("WasiConfig", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            vec![config_entry("k1", "v1"), config_entry("k2", "v2")],
        )
        .await?;

    {
        let result: Vec<(String, String)> = executor
            .invoke_and_await_typed(&component, &agent_id, "get_all", data_value!())
            .await?;

        assert_eq!(
            result,
            string_pairs(&[("k1", "v1"), ("k2", "v2"), ("k3", "v3")])
        )
    }

    let updated_component = executor
        .update_component_with(
            &component.id,
            component.revision,
            None,
            Some(BTreeMap::from([(
                AgentTypeName("WasiConfig".to_string()),
                AgentTypeProvisionConfigUpdate {
                    config: Some(vec![
                        config_entry("k1", "v2"),
                        config_entry("k3", "v4"),
                        config_entry("k4", "v4"),
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
        let mut result: Vec<(String, String)> = executor
            .invoke_and_await_typed(&updated_component, &agent_id, "get_all", data_value!())
            .await?;
        result.sort();

        assert_eq!(
            result,
            string_pairs(&[("k1", "v1"), ("k2", "v2"), ("k3", "v4"), ("k4", "v4")])
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Use a fast response server so the test doesn't take too long
    let server = simulated_slow_request_server(Duration::from_millis(100)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_key!("Clock", "clock-oplog-replay-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: do 3 HTTP requests with 1-second sleeps between them.
    // The sleeps are below the suspend threshold so this completes normally,
    // building up a substantial oplog with subscribe/ready/poll entries.
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 3u64),
        )
        .await?;

    assert_eq!(
        result,
        "Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n"
    );

    info!("First invocation completed, dropping executor to force oplog replay on restart");

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor, which reactivates the worker, but keep the HTTP server running on
    // the same port so the second invocation can actually connect.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker (same component object from before restart):
    // this triggers full oplog replay of the first invocation before switching to live
    // mode for the second. If subscribe() returns different handle IDs during replay,
    // the wstd reactor may diverge and call ready() where poll() was expected.
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 2u64),
        )
        .await?;

    assert_eq!(result2, "Ok(\"slow response\")\nOk(\"slow response\")\n");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Fast response server — each request completes in 100ms
    let server = simulated_slow_request_server(Duration::from_millis(100)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_key!("Clock", "clock-oplog-parallel-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent HTTP request loops (5 requests each) raced with
    // a 30-second timeout. This creates many subscribe/ready/poll calls with complex
    // interleaving across concurrent async tasks in wstd's reactor.
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(30u64),
        )
        .await?;

    // All 3 concurrent loops should complete (each with 5 "slow response" results)
    let line_count = result.lines().count();
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

    // Restart the executor and keep the server running.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay of the
    // parallel HTTP invocation. If subscribe() returns different handle IDs during
    // replay, wstd's reactor HashMap iteration may diverge.
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(30u64),
        )
        .await?;
    let line_count2 = result2.lines().count();
    assert!(
        line_count2 >= 5,
        "Expected at least 5 lines in result, got {}",
        line_count2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);
    Ok(())
}

/// An HTTP client pool with a single connection.
fn single_connection_pool() -> HttpClientConfig {
    HttpClientConfig::Enabled(HttpClientEnabledConfig {
        max_idle_per_host: 1,
        max_connections_per_host: 1,
        max_total_connections: 1,
        ..Default::default()
    })
}

/// A server whose first `/big-byte-array` response streams 100 KiB slowly (about 10 s) and whose
/// later responses return 2 KiB at once.
struct SlowThenFastServer {
    server: LocalHttpServer,
    /// Receives when the slow response starts.
    slow_started: tokio::sync::mpsc::Receiver<()>,
}

async fn slow_then_fast_server() -> anyhow::Result<SlowThenFastServer> {
    let (slow_started_tx, slow_started) = tokio::sync::mpsc::channel::<()>(1);
    let slow_started_tx = Arc::new(slow_started_tx);
    let request_count = Arc::new(AtomicU64::new(0));
    let route = Router::new().route(
        "/big-byte-array",
        get(move || {
            let slow_started_tx = slow_started_tx.clone();
            let request_count = request_count.clone();
            async move {
                let response = Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/octet-stream");
                if request_count.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = slow_started_tx.send(()).await;
                    let stream = stream::iter(0..100)
                        .throttle(Duration::from_millis(100))
                        .map(|_| Ok::<Bytes, BoxError>(Bytes::from(vec![0u8; 1024])));
                    response.body(Body::from_stream(stream)).unwrap()
                } else {
                    response.body(Body::from(vec![0u8; 2048])).unwrap()
                }
            }
        }),
    );
    Ok(SlowThenFastServer {
        server: LocalHttpServer::serve(route).await?,
        slow_started,
    })
}

/// Two unkeyed StreamingClient agents on an executor with a single-connection HTTP pool. The
/// slow agent's request is running and holds the only connection.
struct PoolContention {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    server: LocalHttpServer,
    slow: OwnedAgentId,
    fast: OwnedAgentId,
    slow_invocation: tokio::task::JoinHandle<anyhow::Result<AgentResult>>,
}

impl PoolContention {
    async fn start(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        http_tests: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor =
            start_with_http_client_config(deps, &context, single_connection_pool()).await?;
        let SlowThenFastServer {
            server,
            mut slow_started,
        } = slow_then_fast_server().await?;
        let component = executor
            .component_dep(&context.default_environment_id, http_tests)
            .store()
            .await?;
        let env = HashMap::from([("PORT".to_string(), server.port().to_string())]);
        let slow = executor
            .create_unkeyed_agent(
                &component.id,
                None,
                agent_key!("StreamingClient"),
                env.clone(),
            )
            .await?;
        let fast = executor
            .create_unkeyed_agent(&component.id, None, agent_key!("StreamingClient"), env)
            .await?;
        executor.log_output(&slow).await?;
        executor.log_output(&fast).await?;

        let slow_invocation = spawn({
            let executor = executor.clone();
            let component = component.clone();
            let slow = slow.clone();
            async move {
                executor
                    .invoke_and_await_agent_by_id(
                        &component,
                        &slow,
                        None,
                        "slow_body_stream",
                        data_value!(),
                    )
                    .await
            }
            .in_current_span()
        });
        slow_started
            .recv()
            .await
            .expect("slow stream should have started");
        Ok(Self {
            executor,
            component,
            server,
            slow,
            fast,
            slow_invocation,
        })
    }
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
    let PoolContention {
        executor,
        component,
        server,
        slow,
        fast,
        slow_invocation,
    } = PoolContention::start(deps, last_unique_id, http_tests).await?;

    // The fast agent must wait for the connection pool.
    let fast_start = Instant::now();
    let fast_result = executor
        .invoke_and_await_agent_by_id(&component, &fast, None, "slow_body_stream", data_value!())
        .await?
        .into_typed::<u64>()?;
    let fast_elapsed = fast_start.elapsed();
    info!("Fast agent completed in {fast_elapsed:?}");
    let slow_result = slow_invocation.await??.into_typed::<u64>()?;

    assert!(
        fast_elapsed >= Duration::from_secs(5),
        "Expected fast agent to be blocked by pool contention, but it completed in {fast_elapsed:?}"
    );
    assert_eq!(
        (slow_result, fast_result),
        (100 * 1024, 2048),
        "the slow agent reads the 100 KiB stream, the fast agent the 2 KiB response"
    );

    executor.check_oplog_is_queryable(&slow).await?;
    executor.check_oplog_is_queryable(&fast).await?;

    drop(server);
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
    let PoolContention {
        executor,
        component,
        server,
        slow,
        fast,
        slow_invocation,
    } = PoolContention::start(deps, last_unique_id, http_tests).await?;

    // Phase 1: the pool is contended, so the fast agent's request cannot even start within its
    // two-second timeout.
    let fast_start = Instant::now();
    let fast_result = executor
        .invoke_and_await_agent_by_id(
            &component,
            &fast,
            None,
            "slow_body_stream_with_timeout",
            data_value!(2000u64), // 2 second timeout
        )
        .await?
        .into_typed::<Option<u64>>()?;
    let fast_elapsed = fast_start.elapsed();
    info!("Fast agent completed in {fast_elapsed:?} with result: {fast_result:?}");
    let slow_result = slow_invocation.await??.into_typed::<u64>()?;
    assert_eq!(
        (fast_result, slow_result),
        (None, 100 * 1024),
        "the fast agent times out, the slow agent reads the whole 100 KiB stream"
    );

    // Phase 2: restart to force oplog replay on both agents. Both get the fast response now,
    // because the server already answered its first request.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");
    let slow_result2 = executor
        .invoke_and_await_agent_by_id(&component, &slow, None, "slow_body_stream", data_value!())
        .await?
        .into_typed::<u64>()?;
    let fast_result2 = executor
        .invoke_and_await_agent_by_id(&component, &fast, None, "slow_body_stream", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(
        (slow_result2, fast_result2),
        (2048, 2048),
        "both agents work after replay, including the one that timed out"
    );

    executor.check_oplog_is_queryable(&slow).await?;
    executor.check_oplog_is_queryable(&fast).await?;

    drop(server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

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
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    );
    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    let agent_id = agent_key!("StreamingClient");

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // 1) Loop of timeout calls
    for i in 0..10 {
        let result: Option<u64> = executor
            .invoke_and_await_typed(
                &component,
                &agent_id,
                "slow_body_stream_with_timeout",
                data_value!(0u64),
            )
            .await?;

        info!("Timeout call iteration {i}: {result:?}");
    }

    // 2) Drop executor and restart to force oplog replay

    tokio::time::sleep(Duration::from_secs(2)).await;

    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // 3) Single call without timeout — verifies recovery after replay
    let result = executor
        .invoke_and_await_typed::<u64>(&component, &agent_id, "slow_body_stream", data_value!())
        .await?;

    info!("Post-restart slow_body_stream result: {result:?}");
    assert_eq!(result, 100 * 1024);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(http_server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 50 chunks with 10ms delay between each — generates ~50 poll/ready cycles
    let server = streaming_chunk_server(50, Duration::from_millis(10)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: read a streaming HTTP response with 50 chunks.
    // This builds up a substantial oplog with many subscribe/ready/poll entries.
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "streaming_http_read",
            data_value!(),
        )
        .await?;
    assert!(
        result.contains("chunk-0"),
        "Expected streaming response to contain chunk-0, got: {}",
        result
    );
    assert!(
        result.contains("chunk-49"),
        "Expected streaming response to contain chunk-49, got: {}",
        result
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor but keep the HTTP server running; this triggers oplog replay.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay of the
    // first streaming invocation. If the durable host call sequence diverges
    // during replay, this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "streaming_http_read",
            data_value!(),
        )
        .await?;
    assert!(
        result2.contains("chunk-0"),
        "Expected streaming response to contain chunk-0 in second invocation, got: {}",
        result2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let attempts = Arc::new(AtomicU8::new(0));
    let attempts_clone = attempts.clone();
    let route = Router::new().route(
        "/streaming-chunks",
        get(move || {
            let attempts = attempts_clone.clone();
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
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
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );

    let http_server = LocalHttpServer::serve(route).await?;
    let port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "streaming_http_read",
            data_value!(),
        )
        .await?;

    // The complete second body, with no chunks leaking in from the aborted
    // first attempt (its partial delivery is discarded by the retry's replay).
    let expected: String = (0..20).map(|i| format!("chunk-{i}\n")).collect();
    assert_eq!(
        result, expected,
        "expected exactly the complete re-issued response body"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "the request must have been re-issued exactly once after the mid-body failure"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(http_server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let request_bodies: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_clone = request_bodies.clone();

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
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );

    let http_server = LocalHttpServer::serve(route).await?;
    let host_http_port = http_server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("HttpClient4");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let _handle = executor.spawn_invoke_and_await_with_key(
        &component,
        &agent_id,
        &key,
        "post_with_p3_streamed_body",
        data_value!(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, SHORT_STATUS_TIMEOUT)
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
    drop(http_server);

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Use enough chunks to generate substantial oplog entries
    let server = streaming_chunk_server(20, Duration::from_millis(5)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: streaming HTTP read (goes through trailers path) then sleep.
    // This creates oplog entries for: stream reads + trailers ready/poll + sleep ready/poll.
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "streaming_http_then_sleep",
            data_value!(),
        )
        .await?;
    assert!(
        result.contains("slept"),
        "Expected result to contain 'slept', got: {}",
        result
    );

    info!(
        "First invocation completed: {}, dropping executor to force oplog replay",
        result
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor but keep the HTTP server running; this triggers oplog replay.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay.
    // Previously this would fail with oplog mismatch because FutureTrailers::get()
    // was non-durable. Now that FutureTrailers tracking is properly maintained
    // through the ownership chain, replay should succeed.
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "streaming_http_then_sleep",
            data_value!(),
        )
        .await?;
    assert!(
        result2.contains("slept"),
        "Expected result to contain 'slept', got: {}",
        result2
    );

    drop(server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 30 chunks with 10ms delay — each request generates ~30 poll/ready cycles
    let server = streaming_chunk_server(30, Duration::from_millis(10)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent streaming HTTP reads
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "parallel_streaming_http_reads",
            data_value!(3u64),
        )
        .await?;
    assert!(
        !result.contains("Timeout"),
        "Parallel streaming reads should not have timed out, got: {}",
        result
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation: triggers full oplog replay of the parallel streaming invocation
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "parallel_streaming_http_reads",
            data_value!(3u64),
        )
        .await?;
    assert!(
        !result2.contains("Timeout"),
        "Second invocation should not have timed out, got: {}",
        result2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 50 chunks with 10ms delay between each
    let server = streaming_chunk_server(50, Duration::from_millis(10)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: read a streaming HTTP response using raw WASI APIs
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "raw_streaming_http_read",
            data_value!(),
        )
        .await?;
    assert!(
        result.contains("chunk-0"),
        "Expected streaming response to contain chunk-0, got: {}",
        result
    );
    assert!(
        result.contains("chunk-49"),
        "Expected streaming response to contain chunk-49, got: {}",
        result
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor but keep the HTTP server running; this triggers oplog replay.
    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation on the SAME worker: triggers full oplog replay.
    // If the durable host call sequence diverges during replay,
    // this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "raw_streaming_http_read",
            data_value!(),
        )
        .await?;
    assert!(
        result2.contains("chunk-0"),
        "Expected streaming response to contain chunk-0 in second invocation, got: {}",
        result2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
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
#[timeout("2m")]
async fn oplog_replay_after_parallel_raw_streaming_http_reads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // 30 chunks with 10ms delay — each request generates many poll/ready cycles
    let server = streaming_chunk_server(30, Duration::from_millis(10)).await;
    let port = server.port();

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_key!("StreamingClient");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    // First invocation: 3 concurrent raw streaming HTTP reads, each doing 3 sequential requests
    let result = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "parallel_raw_streaming_http_reads",
            data_value!(3u64),
        )
        .await?;
    assert!(
        !result.contains("Timeout"),
        "Parallel raw streaming reads should not have timed out, got: {}",
        result
    );

    info!(
        "First invocation completed ({} bytes), dropping executor to force oplog replay",
        result.len()
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    info!("Restarting executor...");
    let executor = executor.restart(Teardown::Drop).await?;
    info!("Executor restarted");

    // Second invocation: triggers full oplog replay of the parallel raw streaming invocation.
    // If the durable host call sequence diverges during replay,
    // this will fail with "expected io::poll::poll, got io::poll::ready".
    let result2 = executor
        .invoke_and_await_typed::<String>(
            &component,
            &agent_id,
            "parallel_raw_streaming_http_reads",
            data_value!(3u64),
        )
        .await?;
    assert!(
        !result2.contains("Timeout"),
        "Second invocation should not have timed out, got: {}",
        result2
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(server);
    drop(executor);
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux;
