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

//! Filesystem snapshots through the executor: a snapshot and a manual update keep the files of an
//! agent across a restart, and a restart from a snapshot gives the files that a full replay gives.

use crate::Tracing;
use crate::durability::assert_snapshot_recovery_loaded;
use anyhow::anyhow;
use futures::{StreamExt as _, TryStreamExt as _};
use golem_common::base_model::component::ComponentDto;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath, ComponentRevision};
use golem_common::model::oplog::{
    MultipartPartData, OplogEntry, OplogPayload, PublicOplogEntry, PublicSnapshotData,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationPayload, OplogIndex, OwnedAgentId,
};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::model::IFSEntry;
use golem_worker_executor::filesystem_snapshot_testing::TestFilesystemSnapshotStore;
use golem_worker_executor::services::golem_config::{
    FilesystemSnapshotStoreConfig, FilesystemSnapshotUploadConfig, FilesystemSnapshotUploadValues,
    FilesystemSnapshotsConfig, SnapshotPolicy,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("initial_file_system")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("constructor_parameter_echo")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The agent type of the test component that takes snapshots.
const AGENT_TYPE: &str = "SnapshotTree";

/// The upload settings of the tests: one attempt for each save, so an injected failure ends the
/// upload at once, and a short wait of a start for an upload.
fn uploads(confirmation_wait: Duration) -> FilesystemSnapshotUploadConfig {
    FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
        max_concurrent_uploads: 4,
        max_concurrent_restores: 4,
        confirmation_wait,
        store_check_limit: Duration::from_secs(5),
        capture_wait: Duration::from_secs(5),
        retained_periodic_snapshots: 2,
        retained_update_snapshots: 2,
        upload_retry: golem_common::model::RetryConfig {
            max_attempts: 1,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(10),
            multiplier: 2.0,
            max_jitter_factor: None,
        },
    })
    .expect("valid upload settings")
}

/// Starts an executor that takes a snapshot after each invocation and keeps filesystem snapshots
/// in `store`. With `root`, the agent filesystems live under it.
async fn start_snapshotting(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    store: &TestFilesystemSnapshotStore,
    confirmation_wait: Duration,
    root: Option<&Path>,
) -> anyhow::Result<TestWorkerExecutor> {
    let root: Option<Box<Path>> = root.map(Box::from);
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.deterministic_root_dir =
                    root.as_deref().map(Path::to_path_buf);
                config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
            })),
            filesystem_snapshot_store: Some((store.clone(), uploads(confirmation_wait))),
            ..TestExecutorOverrides::default()
        },
    )
    .await
}

/// Starts an executor without snapshots, whose agent filesystems live under the empty `root`. A
/// start of an agent then replays its whole oplog.
async fn start_replaying(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    root: &Path,
) -> anyhow::Result<TestWorkerExecutor> {
    start_replaying_with(deps, context, root, None).await
}

/// Starts an executor that takes no snapshots, whose agent filesystems live under the empty
/// `root`, and that restores filesystem snapshots from `store` when it is given.
async fn start_replaying_with(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    root: &Path,
    store: Option<&TestFilesystemSnapshotStore>,
) -> anyhow::Result<TestWorkerExecutor> {
    let root: Box<Path> = Box::from(root);
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.deterministic_root_dir = Some(root.to_path_buf());
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
                config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
            })),
            filesystem_snapshot_store: store
                .map(|store| (store.clone(), FilesystemSnapshotUploadConfig::default())),
            ..TestExecutorOverrides::default()
        },
    )
    .await
}

fn entry(source: &str, target: &str, permissions: AgentFilePermissions) -> IFSEntry {
    IFSEntry {
        source_path: PathBuf::from(format!("initial-file-system/files/{source}")),
        target_path: CanonicalFilePath::from_abs_str(target).expect("valid path"),
        permissions,
    }
}

/// One filesystem operation of the test agent, relative to the root of its filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Mkdir {
        path: &'static str,
    },
    Write {
        path: &'static str,
        content: &'static str,
    },
    Remove {
        path: &'static str,
    },
    Rename {
        from: &'static str,
        to: &'static str,
    },
    Link {
        existing: &'static str,
        link: &'static str,
    },
    Symlink {
        link: &'static str,
        target: &'static str,
    },
}

impl Operation {
    /// The operation, the path and the argument of the `apply` call of the agent.
    fn call(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Mkdir { path } => ("mkdir", path, ""),
            Self::Write { path, content } => ("write", path, content),
            Self::Remove { path } => ("remove", path, ""),
            Self::Rename { from, to } => ("rename", from, to),
            Self::Link { existing, link } => ("link", existing, link),
            Self::Symlink { link, target } => ("symlink", link, target),
        }
    }
}

/// The agent under test, with its component.
struct Agent {
    component: ComponentDto,
    agent: ParsedAgentId,
    worker_id: AgentId,
}

impl Agent {
    /// Stores the component with `files` as the initial files of the agent type, and starts the
    /// agent `name`.
    async fn start(
        executor: &TestWorkerExecutor,
        context: &TestContext,
        component: &PrecompiledComponent,
        name: &str,
        files: &[IFSEntry],
    ) -> anyhow::Result<Self> {
        let component = executor
            .component_dep(&context.default_environment_id, component)
            .with_files(AGENT_TYPE, files)
            .store()
            .await?;
        let agent = agent_id!(AGENT_TYPE, name);
        let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
        Ok(Self {
            component,
            agent,
            worker_id,
        })
    }

    fn owned(&self, context: &TestContext) -> OwnedAgentId {
        OwnedAgentId::new(context.default_environment_id, &self.worker_id)
    }

    /// The agent and the fingerprint of its incarnation, which the `Create` entry of its oplog
    /// holds. The filesystem snapshots of the agent are keyed by both.
    async fn incarnation(
        &self,
        executor: &TestWorkerExecutor,
        context: &TestContext,
    ) -> anyhow::Result<(OwnedAgentId, AgentFingerprint)> {
        let fingerprint = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .find_map(|entry| match entry.entry {
                PublicOplogEntry::Create(create) => Some(AgentFingerprint(create.instance_id)),
                _ => None,
            })
            .ok_or_else(|| anyhow!("the oplog of the agent has no create entry"))?;
        Ok((self.owned(context), fingerprint))
    }

    /// Applies `operation` and gives its result: `ok`, or the error of the agent.
    async fn apply(
        &self,
        executor: &TestWorkerExecutor,
        operation: Operation,
    ) -> anyhow::Result<String> {
        let (operation, path, argument) = operation.call();
        let (operation, path, argument) = (
            operation.to_string(),
            path.to_string(),
            argument.to_string(),
        );
        match executor
            .invoke_and_await_agent(
                &self.component,
                &self.agent,
                "apply",
                data_value!(operation, path, argument),
            )
            .await?
            .into_return_value()
        {
            Some(SchemaValue::String(outcome)) => Ok(outcome),
            other => Err(anyhow!("apply gave {other:?}")),
        }
    }

    /// Applies each operation and requires `ok`.
    async fn apply_all(
        &self,
        executor: &TestWorkerExecutor,
        operations: &[Operation],
    ) -> anyhow::Result<()> {
        let outcomes = futures::stream::iter(operations.iter())
            .then(|operation| self.apply(executor, *operation))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;
        if outcomes.iter().all(|outcome| outcome == "ok") {
            Ok(())
        } else {
            Err(anyhow!("the operations gave {outcomes:?}"))
        }
    }

    async fn describe(&self, executor: &TestWorkerExecutor) -> anyhow::Result<Vec<String>> {
        match executor
            .invoke_and_await_agent(&self.component, &self.agent, "describe", data_value!())
            .await?
            .into_return_value()
        {
            Some(SchemaValue::List { elements }) => elements
                .into_iter()
                .map(|element| match element {
                    SchemaValue::String(line) => Ok(line),
                    other => Err(anyhow!("describe gave {other:?}")),
                })
                .collect(),
            other => Err(anyhow!("describe gave {other:?}")),
        }
    }

    async fn applied(&self, executor: &TestWorkerExecutor) -> anyhow::Result<u32> {
        executor
            .invoke_and_await_agent(&self.component, &self.agent, "applied", data_value!())
            .await?
            .into_typed::<u32>()
    }

    /// The filesystem snapshot names of the snapshot records, and the names of the confirmation
    /// records, in oplog order.
    async fn records(&self, executor: &TestWorkerExecutor) -> anyhow::Result<Records> {
        let oplog = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        let (snapshots, confirmations, failed_updates) = oplog.into_iter().fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |(mut snapshots, mut confirmations, mut failed_updates), entry| {
                match entry.entry {
                    PublicOplogEntry::Snapshot(snapshot) => {
                        snapshots.push(snapshot.filesystem_snapshot.map(String::into_boxed_str))
                    }
                    PublicOplogEntry::SnapshotConfirmed(confirmed) => {
                        confirmations.push(confirmed.filesystem_snapshot.into_boxed_str())
                    }
                    PublicOplogEntry::FailedUpdate(failed) => {
                        failed_updates.push(failed.details.unwrap_or_default().into_boxed_str())
                    }
                    _ => {}
                }
                (snapshots, confirmations, failed_updates)
            },
        );
        Ok(Records {
            snapshots: snapshots.into_boxed_slice(),
            confirmations: confirmations.into_boxed_slice(),
            failed_updates: failed_updates.into_boxed_slice(),
        })
    }

    /// Waits until the confirmation of the newest snapshot record with a name is in the oplog,
    /// and gives the name.
    async fn confirmed(&self, executor: &TestWorkerExecutor) -> anyhow::Result<String> {
        eventually(Duration::from_secs(30), || async {
            let records = self.records(executor).await?;
            Ok(records.newest_name().filter(|name| {
                records.confirmations.last().map(|confirmed| &**confirmed) == Some(name.as_str())
            }))
        })
        .await
    }

    /// Waits until the oplog holds a failed update, and gives the details of each one.
    async fn failed_updates(&self, executor: &TestWorkerExecutor) -> anyhow::Result<Vec<String>> {
        eventually(Duration::from_secs(60), || async {
            let records = self.records(executor).await?;
            Ok((!records.failed_updates.is_empty()).then(|| {
                records
                    .failed_updates
                    .iter()
                    .map(|details| details.to_string())
                    .collect()
            }))
        })
        .await
    }

    /// Applies `operation` and waits until the snapshot record that it wrote is confirmed. Gives
    /// the index of the record and its name.
    async fn apply_and_confirm(
        &self,
        executor: &TestWorkerExecutor,
        operation: Operation,
    ) -> anyhow::Result<(OplogIndex, String)> {
        let before = self.records(executor).await?.snapshots.len();
        self.apply_all(executor, &[operation]).await?;
        // The snapshot record of the operation comes after the invocation ends, so the wait
        // looks only at the records that the oplog did not hold before the operation.
        let name = eventually(Duration::from_secs(30), || async {
            let records = self.records(executor).await?;
            Ok(records
                .snapshots
                .get(before..)
                .and_then(|newer| newer.iter().rev().flatten().next())
                .filter(|name| records.is_confirmed(name))
                .map(|name| name.to_string()))
        })
        .await?;
        let oplog = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        oplog
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Snapshot(snapshot)
                    if snapshot.filesystem_snapshot.as_ref() == Some(&name) =>
                {
                    Some((entry.oplog_index, name.clone()))
                }
                _ => None,
            })
            .ok_or_else(|| anyhow!("no snapshot record has the name {name}"))
    }

    /// Stops the agent, starts it again and gives the kinds of the oplog entries that the start
    /// and one `describe` wrote up to the end of `describe`, and the tree that `describe` gave.
    async fn restart_and_describe(
        &self,
        executor: &TestWorkerExecutor,
        context: &TestContext,
    ) -> anyhow::Result<(Vec<String>, Vec<String>)> {
        self.stop(executor, context).await?;
        let before = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?
            .len();
        executor.resume(&self.worker_id, false).await?;
        let tree = self.describe(executor).await?;
        let kinds = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .skip(before)
            .map(|entry| entry_kind(&entry.entry))
            .scan(false, |finished, kind| {
                (!*finished).then(|| {
                    *finished = kind == "AgentInvocationFinished";
                    kind
                })
            })
            .collect();
        Ok((kinds, tree))
    }

    /// Stops the agent once it is idle.
    async fn stop(
        &self,
        executor: &TestWorkerExecutor,
        context: &TestContext,
    ) -> anyhow::Result<()> {
        eventually(Duration::from_secs(10), || async {
            Ok(executor
                .stop_worker_if_idle(&self.owned(context))
                .await?
                .then_some(()))
        })
        .await
    }

    /// Asks for a manual update to a new revision with `files`, and waits until the oplog holds
    /// one more update outcome. Gives the new component.
    async fn manual_update(
        &self,
        executor: &TestWorkerExecutor,
        files: Vec<IFSEntry>,
    ) -> anyhow::Result<ComponentDto> {
        let outcomes = self.update_results(executor).await?.len();
        let updated = executor
            .update_component_with_files(
                &self.component.id,
                AGENT_TYPE,
                "it_initial_file_system_release",
                files,
            )
            .await?;
        executor
            .manual_update_worker(&self.worker_id, updated.revision, false)
            .await?;
        eventually(Duration::from_secs(60), || async {
            Ok((self.update_results(executor).await?.len() > outcomes).then_some(()))
        })
        .await?;
        Ok(updated)
    }

    /// The outcome of each update in the oplog, in oplog order.
    async fn update_results(&self, executor: &TestWorkerExecutor) -> anyhow::Result<Vec<String>> {
        Ok(executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .filter_map(|entry| match entry.entry {
                PublicOplogEntry::SuccessfulUpdate(updated) => {
                    Some(format!("updated to {:?}", updated.target_revision))
                }
                PublicOplogEntry::FailedUpdate(failed) => Some(format!(
                    "failed to update to {:?}: {}",
                    failed.target_revision,
                    failed.details.unwrap_or_default()
                )),
                _ => None,
            })
            .collect())
    }
}

/// The name of the kind of `entry`, without its fields.
fn entry_kind(entry: &PublicOplogEntry) -> String {
    let text = format!("{entry:?}");
    text.split(['(', ' ', '{'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// What the oplog of an agent holds about snapshots, in oplog order.
#[derive(Debug, Default)]
struct Records {
    snapshots: Box<[Option<Box<str>>]>,
    confirmations: Box<[Box<str>]>,
    failed_updates: Box<[Box<str>]>,
}

impl Records {
    /// The filesystem snapshot name of the newest snapshot record with a name.
    fn newest_name(&self) -> Option<String> {
        self.snapshots
            .iter()
            .rev()
            .flatten()
            .next()
            .map(|name| name.to_string())
    }

    /// The filesystem snapshot name of the oldest snapshot record with a name.
    fn oldest_name(&self) -> Option<String> {
        self.snapshots
            .iter()
            .flatten()
            .next()
            .map(|name| name.to_string())
    }

    /// Whether a confirmation record confirms `name`.
    fn is_confirmed(&self, name: &str) -> bool {
        self.confirmations
            .iter()
            .any(|confirmed| &**confirmed == name)
    }
}

/// Calls `check` until it gives a value, for at most `limit`.
async fn eventually<T, Check, Checking>(limit: Duration, check: Check) -> anyhow::Result<T>
where
    Check: Fn() -> Checking,
    Checking: std::future::Future<Output = anyhow::Result<Option<T>>>,
{
    let checks = futures::stream::repeat(())
        .then(|()| async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            check().await
        })
        .filter_map(|checked| std::future::ready(checked.transpose()));
    tokio::time::timeout(limit, std::pin::pin!(checks).next())
        .await
        .map_err(|_| anyhow!("the condition did not hold within {limit:?}"))?
        .ok_or_else(|| anyhow!("the checks ended"))?
}

#[test]
#[timeout("4m")]
async fn a_periodic_snapshot_brings_the_files_back_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "periodic", &[]).await?;

    agent
        .apply_all(
            &executor,
            &[
                Operation::Mkdir { path: "data" },
                Operation::Write {
                    path: "data/one.txt",
                    content: "one",
                },
                Operation::Write {
                    path: "two.txt",
                    content: "two",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let live = agent.describe(&executor).await?;
    let applied = agent.applied(&executor).await?;
    executor.release().await?;
    let restores = store.restored_names().len();

    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let restored = agent.describe(&restarted).await?;

    assert_eq!(restored, live);
    assert_eq!(agent.applied(&restarted).await?, applied);
    assert!(store.restored_names().len() > restores);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "replay-equal",
        &[
            entry("foo.txt", "/ro-kept.txt", AgentFilePermissions::ReadOnly),
            entry("foo.txt", "/ro-deleted.txt", AgentFilePermissions::ReadOnly),
            entry("foo.txt", "/ro-renamed.txt", AgentFilePermissions::ReadOnly),
            entry("bar.txt", "/ro-linked.txt", AgentFilePermissions::ReadOnly),
            entry(
                "bar.txt",
                "/dir/ro-in-dir.txt",
                AgentFilePermissions::ReadOnly,
            ),
            entry(
                "baz.txt",
                "/rw-modified.txt",
                AgentFilePermissions::ReadWrite,
            ),
            entry(
                "baz.txt",
                "/rw-deleted.txt",
                AgentFilePermissions::ReadWrite,
            ),
        ],
    )
    .await?;

    agent
        .apply_all(
            &executor,
            &[
                Operation::Remove {
                    path: "ro-deleted.txt",
                },
                Operation::Write {
                    path: "rw-modified.txt",
                    content: "modified by the agent",
                },
                Operation::Remove {
                    path: "rw-deleted.txt",
                },
                Operation::Rename {
                    from: "ro-renamed.txt",
                    to: "renamed.txt",
                },
                Operation::Link {
                    existing: "ro-linked.txt",
                    link: "second-name.txt",
                },
                Operation::Rename {
                    from: "dir",
                    to: "moved-dir",
                },
                Operation::Write {
                    path: "agent.txt",
                    content: "agent data",
                },
                Operation::Link {
                    existing: "agent.txt",
                    link: "agent-alias.txt",
                },
                Operation::Symlink {
                    link: "pointer",
                    target: "agent.txt",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let live = agent.describe(&executor).await?;
    executor.release().await?;

    let from_snapshot =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let restored = agent.describe(&from_snapshot).await?;
    from_snapshot.release().await?;
    let replay_root = tempfile::tempdir()?;
    let replaying = start_replaying(deps, &context, replay_root.path()).await?;
    let replayed = agent.describe(&replaying).await?;

    assert!(!store.restored_names().is_empty());
    assert_eq!(restored, live);
    assert_eq!(replayed, live);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn an_injected_upload_failure_falls_back_and_a_later_upload_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor = start_snapshotting(deps, &context, &store, Duration::from_secs(1), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "failure", &[]).await?;
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "first.txt",
                content: "first",
            }],
        )
        .await?;
    let first = agent.confirmed(&executor).await?;

    store.fail_next_saves(usize::MAX);
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "second.txt",
                content: "second",
            }],
        )
        .await?;
    let saves = store.save_count();
    eventually(Duration::from_secs(30), || async {
        Ok((store.save_count() > saves).then_some(()))
    })
    .await?;
    let failed = agent.records(&executor).await?;
    let live = agent.describe(&executor).await?;
    executor.release().await?;

    let falling_back =
        start_snapshotting(deps, &context, &store, Duration::from_secs(1), None).await?;
    let after_failure = agent.describe(&falling_back).await?;
    store.fail_next_saves(0);
    agent
        .apply_all(
            &falling_back,
            &[Operation::Write {
                path: "third.txt",
                content: "third",
            }],
        )
        .await?;
    let recovered = agent.confirmed(&falling_back).await?;
    let recovered_live = agent.describe(&falling_back).await?;
    falling_back.release().await?;
    let restores = store.restored_names().len();
    let recovering =
        start_snapshotting(deps, &context, &store, Duration::from_secs(1), None).await?;
    let after_recovery = agent.describe(&recovering).await?;

    assert_eq!(
        failed.confirmations.last().map(|confirmed| &**confirmed),
        Some(first.as_str())
    );
    assert_ne!(
        failed.newest_name(),
        Some(first.clone()),
        "the failed upload wrote its own record"
    );
    assert_eq!(after_failure, live);
    assert_ne!(recovered, first);
    assert_eq!(after_recovery, recovered_live);
    assert!(store.restored_names().len() > restores);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn invocations_during_an_upload_keep_their_results_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "during", &[]).await?;
    store.set_save_delay(Duration::from_secs(2));

    agent
        .apply_all(
            &executor,
            &[
                Operation::Write {
                    path: "before.txt",
                    content: "before",
                },
                Operation::Write {
                    path: "during-1.txt",
                    content: "during",
                },
                Operation::Write {
                    path: "during-2.txt",
                    content: "during",
                },
            ],
        )
        .await?;
    let live = agent.describe(&executor).await?;
    let applied = agent.applied(&executor).await?;
    agent.confirmed(&executor).await?;
    store.set_save_delay(Duration::ZERO);
    executor.release().await?;

    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;

    assert_eq!(agent.describe(&restarted).await?, live);
    assert_eq!(agent.applied(&restarted).await?, applied);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn the_next_start_confirms_the_snapshot_of_an_agent_that_stopped_during_its_upload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "stopping", &[]).await?;
    let (live, named) = stop_during_an_upload(&executor, &context, &store, &agent).await?;
    let while_stopped = agent.records(&executor).await?;
    let restores = store.restored_names().len();

    executor.resume(&agent.worker_id, false).await?;
    let after_start = agent.describe(&executor).await?;
    let records = agent.records(&executor).await?;

    assert!(!while_stopped.is_confirmed(&named), "{while_stopped:?}");
    assert!(records.is_confirmed(&named), "{records:?}");
    assert_eq!(after_start, live);
    assert!(store.restored_names().len() > restores);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_start_on_another_executor_confirms_a_whole_snapshot_and_restores_it(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "moved", &[]).await?;
    let (live, named) = stop_during_an_upload(&executor, &context, &store, &agent).await?;
    executor.release().await?;
    let restores = store.restored_names().len();

    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    restarted.resume(&agent.worker_id, false).await?;
    let after_start = agent.describe(&restarted).await?;
    let records = agent.records(&restarted).await?;

    assert!(records.is_confirmed(&named), "{records:?}");
    assert_eq!(after_start, live);
    assert!(store.restored_names().len() > restores);
    Ok(())
}

/// Changes the tree of `agent` with an upload that takes two seconds, stops the agent while the
/// upload runs, and waits until the store holds the whole snapshot and the upload ended. Gives the
/// tree before the stop and the name of the snapshot.
async fn stop_during_an_upload(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    store: &TestFilesystemSnapshotStore,
    agent: &Agent,
) -> anyhow::Result<(Vec<String>, String)> {
    store.set_save_delay(Duration::from_secs(2));
    agent
        .apply_all(
            executor,
            &[Operation::Write {
                path: "file.txt",
                content: "content",
            }],
        )
        .await?;
    let live = agent.describe(executor).await?;
    let named = eventually(Duration::from_secs(30), || async {
        let records = agent.records(executor).await?;
        Ok(records.newest_name())
    })
    .await?;
    let (owned, fingerprint) = agent.incarnation(executor, context).await?;
    agent.stop(executor, context).await?;
    eventually(Duration::from_secs(30), || async {
        Ok(store
            .snapshot_names(&owned, fingerprint)
            .await
            .contains(&named)
            .then_some(()))
    })
    .await?;
    store.set_save_delay(Duration::ZERO);
    // The upload asks for its confirmation right after the save; give it time to get its answer.
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok((live, named))
}

#[test]
#[timeout("4m")]
async fn a_crash_during_an_upload_restarts_with_the_tree_of_a_run_without_a_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let operations = [
        Operation::Mkdir { path: "logs" },
        Operation::Write {
            path: "logs/1.txt",
            content: "one",
        },
        Operation::Write {
            path: "logs/2.txt",
            content: "two",
        },
        Operation::Rename {
            from: "logs/1.txt",
            to: "logs/first.txt",
        },
        Operation::Link {
            existing: "logs/2.txt",
            link: "second.txt",
        },
        Operation::Remove { path: "logs/2.txt" },
    ];
    let reference_context = TestContext::new(last_unique_id);
    let reference_store = TestFilesystemSnapshotStore::new();
    let reference_executor = start_snapshotting(
        deps,
        &reference_context,
        &reference_store,
        Duration::from_secs(30),
        None,
    )
    .await?;
    let reference = Agent::start(
        &reference_executor,
        &reference_context,
        initial_file_system,
        "reference",
        &[],
    )
    .await?;
    reference
        .apply_all(&reference_executor, &operations)
        .await?;
    let expected = reference.describe(&reference_executor).await?;
    reference_executor.release().await?;

    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "crashing", &[]).await?;
    let (before, last) = operations.split_at(operations.len() - 1);
    agent.apply_all(&executor, before).await?;
    let confirmed = agent.confirmed(&executor).await?;
    let held = store.hold_next_save();
    agent.apply_all(&executor, last).await?;
    // The snapshot of the last operation can be skipped, when the job before it still deletes
    // older snapshots at that moment. A `describe` changes no file and takes the snapshot then.
    let blocked = eventually(Duration::from_secs(30), || async {
        match held.name() {
            Some(name) => Ok(Some(name)),
            None => agent.describe(&executor).await.map(|_| None),
        }
    })
    .await?;
    let newest = agent.records(&executor).await?.newest_name();
    // The crash: the executor goes away while the upload of the last snapshot is held.
    executor.release().await?;
    drop(held);
    let restores = store.restored_names().len();

    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let tree = agent.describe(&restarted).await?;
    let restored = store.restored_names()[restores..].to_vec();
    let records = agent.records(&restarted).await?;

    let shape = invocation_shape(&restarted.stored_oplog(&agent.worker_id).await);

    assert_eq!(newest.as_ref(), Some(&blocked));
    assert!(
        !records.is_confirmed(&blocked),
        "the held snapshot {blocked} was confirmed: {records:?}"
    );
    assert_eq!(restored, [confirmed]);
    assert_eq!(tree, expected);
    assert_eq!(
        shape,
        InvocationShape {
            not_finished_once: Box::default(),
            starts_without_terminal: Box::default(),
            durable_calls_between_invocations: Box::default(),
            applied: operations.len(),
        }
    );
    Ok(())
}

/// The shape of the invocation entries of an oplog after a restart.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvocationShape {
    /// Each idempotency key whose invocation has no `AgentInvocationFinished`, or more than one.
    pub(crate) not_finished_once: Box<[Box<str>]>,
    /// The index of each durable-call `Start` without an `End` or a `Cancelled`.
    pub(crate) starts_without_terminal: Box<[u64]>,
    /// The index of each durable-call `Start`, `End` or `Cancelled` in a gap between
    /// invocations: after an `AgentInvocationFinished` and before the next
    /// `AgentInvocationStarted`, or the end.
    pub(crate) durable_calls_between_invocations: Box<[u64]>,
    /// The number of finished `apply` invocations.
    pub(crate) applied: usize,
}

impl InvocationShape {
    /// The shape of a settled oplog without `apply` invocations: each invocation finished once,
    /// each durable call ended, and no durable call between invocations.
    pub(crate) fn settled() -> Self {
        Self {
            not_finished_once: Box::default(),
            starts_without_terminal: Box::default(),
            durable_calls_between_invocations: Box::default(),
            applied: 0,
        }
    }
}

/// Reads the shape of the invocation entries of `oplog`, whose first entry is at index 1. An
/// `AgentInvocationFinished` belongs to the `AgentInvocationStarted` before it.
pub(crate) fn invocation_shape(oplog: &[OplogEntry]) -> InvocationShape {
    let (finished, _) = oplog.iter().fold(
        (std::collections::BTreeMap::<String, usize>::new(), None),
        |(mut finished, current), entry| match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                let key = idempotency_key.to_string();
                finished.entry(key.clone()).or_insert(0);
                (finished, Some(key))
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                *finished
                    .entry(current.unwrap_or_else(|| "<no started invocation>".to_string()))
                    .or_insert(0) += 1;
                (finished, None)
            }
            _ => (finished, current),
        },
    );
    let indexed = || (1u64..).zip(oplog.iter());
    let terminated = indexed()
        .filter_map(|(_, entry)| match entry {
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                Some(u64::from(*start_index))
            }
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    InvocationShape {
        not_finished_once: finished
            .into_iter()
            .filter(|(_, count)| *count != 1)
            .map(|(key, count)| format!("{key}: {count}").into_boxed_str())
            .collect(),
        starts_without_terminal: indexed()
            .filter(|(index, entry)| {
                matches!(entry, OplogEntry::Start { .. }) && !terminated.contains(index)
            })
            .map(|(index, _)| index)
            .collect(),
        durable_calls_between_invocations: indexed()
            .scan(false, |between, (index, entry)| {
                let durable_call = *between
                    && matches!(
                        entry,
                        OplogEntry::Start { .. }
                            | OplogEntry::End { .. }
                            | OplogEntry::Cancelled { .. }
                    );
                match entry {
                    OplogEntry::AgentInvocationFinished { .. } => *between = true,
                    OplogEntry::AgentInvocationStarted { .. } => *between = false,
                    _ => {}
                }
                Some(durable_call.then_some(index))
            })
            .flatten()
            .collect(),
        applied: oplog
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    OplogEntry::AgentInvocationFinished { method_name: Some(name), .. }
                        if name == "apply"
                )
            })
            .count(),
    }
}

#[test]
#[timeout("4m")]
async fn a_tree_of_initial_files_writes_records_without_a_name_and_uses_no_store(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let files = [
        entry("foo.txt", "/ro-top.txt", AgentFilePermissions::ReadOnly),
        entry(
            "bar.txt",
            "/nested/ro-deep.txt",
            AgentFilePermissions::ReadOnly,
        ),
    ];
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "initial-files",
        &files,
    )
    .await?;

    let live = agent.describe(&executor).await?;
    agent.applied(&executor).await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok((records.snapshots.len() >= 2).then_some(()))
    })
    .await?;
    let records = agent.records(&executor).await?;
    let unchanged = executor.filesystem_captures("unchanged");
    executor.release().await?;
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let restarted = agent.describe(&executor).await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![entry(
                "foo.txt",
                "/ro-top.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    let updated_agent = Agent {
        component: updated,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let after_update = updated_agent.describe(&executor).await?;
    executor.release().await?;
    let replay_root = tempfile::tempdir()?;
    let replaying = start_replaying(deps, &context, replay_root.path()).await?;
    let replayed = updated_agent.describe(&replaying).await?;

    assert!(records.snapshots.iter().all(Option::is_none), "{records:?}");
    assert!(records.confirmations.is_empty());
    assert_eq!(store.save_count(), 0);
    // The executor runs only this agent, so its count is the count of this agent. The boundary
    // after the first record without a name finds the tree unchanged against the mark of that
    // record, and checks no declaration and no directory.
    assert!(unchanged >= 1, "{unchanged} unchanged captures");
    assert_eq!(
        live,
        [
            "nested dir",
            r#"nested/ro-deep.txt file links=1 writable=false content="bar\n""#,
            r#"ro-top.txt file links=1 writable=false content="foo\n""#,
        ]
        .map(String::from)
    );
    // A restart from a record without a name seeds the initial files, as a full replay does.
    assert_eq!(restarted, live);
    // The update keeps the directory that held the file that it removed, as a replay of the
    // update does.
    assert_eq!(
        after_update,
        [
            "nested dir",
            r#"ro-top.txt file links=1 writable=false content="foo\n""#,
        ]
        .map(String::from)
    );
    assert_eq!(replayed, after_update);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn an_unchanged_tree_reuses_the_confirmed_name_and_saves_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "unchanged", &[]).await?;
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "file.txt",
                content: "content",
            }],
        )
        .await?;
    let name = agent.confirmed(&executor).await?;
    let saves = store.save_count();
    let snapshots_before = agent.records(&executor).await?.snapshots.len();

    let live = agent.describe(&executor).await?;
    agent.applied(&executor).await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok((records.snapshots.len() >= snapshots_before + 2).then_some(records))
    })
    .await?;
    let records = agent.records(&executor).await?;
    executor.release().await?;
    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;

    assert_eq!(store.save_count(), saves);
    assert!(
        records.snapshots[snapshots_before..]
            .iter()
            .all(|snapshot| snapshot.as_deref() == Some(name.as_str())),
        "{records:?}"
    );
    assert_eq!(
        records
            .confirmations
            .iter()
            .filter(|confirmed| ***confirmed == *name)
            .count(),
        records.snapshots.len() - snapshots_before + 1
    );
    assert_eq!(agent.describe(&restarted).await?, live);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_manual_update_brings_the_files_into_the_target_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "manual-update",
        &[entry(
            "foo.txt",
            "/ro-old.txt",
            AgentFilePermissions::ReadOnly,
        )],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Mkdir { path: "work" },
                Operation::Write {
                    path: "work/state.db",
                    content: "rows",
                },
            ],
        )
        .await?;
    // Let the periodic upload end first, so the manual update is admitted at once and does not
    // wait for the running upload.
    agent.confirmed(&executor).await?;
    let saves = store.save_count();

    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![entry(
                "bar.txt",
                "/ro-new.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    let updated_agent = Agent {
        component: updated,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let after_update = updated_agent.describe(&executor).await?;
    executor.release().await?;
    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;

    assert!(store.save_count() > saves);
    assert_eq!(
        after_update,
        [
            r#"ro-new.txt file links=1 writable=false content="bar\n""#,
            "work dir",
            r#"work/state.db file links=1 writable=true content="rows""#,
        ]
        .map(String::from)
    );
    assert_eq!(updated_agent.describe(&restarted).await?, after_update);
    Ok(())
}

#[test]
#[timeout("6m")]
async fn a_changed_file_at_a_changed_declaration_fails_both_updates_the_same_way(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let files = [entry(
        "baz.txt",
        "/config.txt",
        AgentFilePermissions::ReadWrite,
    )];
    let manual = Agent::start(&executor, &context, initial_file_system, "manual", &files).await?;
    let automatic = Agent {
        worker_id: executor
            .start_agent(&manual.component.id, agent_id!(AGENT_TYPE, "automatic"))
            .await?,
        agent: agent_id!(AGENT_TYPE, "automatic"),
        component: manual.component.clone(),
    };
    manual
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "config.txt",
                content: "changed",
            }],
        )
        .await?;
    automatic
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "config.txt",
                content: "changed",
            }],
        )
        .await?;
    let manual_before = manual.describe(&executor).await?;
    let automatic_before = automatic.describe(&executor).await?;
    let updated = executor
        .update_component_with_files(
            &manual.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![entry(
                "foo.txt",
                "/config.txt",
                AgentFilePermissions::ReadWrite,
            )],
        )
        .await?;

    executor
        .manual_update_worker(&manual.worker_id, updated.revision, false)
        .await?;
    executor
        .auto_update_worker(&automatic.worker_id, updated.revision, false)
        .await?;
    let manual_failures = manual.failed_updates(&executor).await?;
    let automatic_failures = automatic.failed_updates(&executor).await?;

    assert!(
        manual_failures
            .iter()
            .any(|failure| failure.contains("config.txt")),
        "{manual_failures:?}"
    );
    assert!(
        automatic_failures
            .iter()
            .any(|failure| failure.contains("config.txt")),
        "{automatic_failures:?}"
    );
    assert_eq!(manual.describe(&executor).await?, manual_before);
    // A failed automatic update ends the start with an error, and the agent starts again at its
    // current revision after the retry delay.
    let automatic_after = eventually(Duration::from_secs(60), || async {
        Ok(automatic.describe(&executor).await.ok())
    })
    .await?;
    assert_eq!(automatic_after, automatic_before);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_start_waits_for_a_running_upload_until_the_limit_and_then_falls_back(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let limit = Duration::from_secs(3);
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor = start_snapshotting(deps, &context, &store, limit, None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "waiting", &[]).await?;
    store.set_save_delay(Duration::from_secs(120));
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "file.txt",
                content: "content",
            }],
        )
        .await?;
    let live = agent.describe(&executor).await?;
    let named = eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.oldest_name())
    })
    .await?;
    agent.stop(&executor, &context).await?;
    let restores = store.restored_names().len();

    let started = std::time::Instant::now();
    executor.resume(&agent.worker_id, false).await?;
    let after_start = agent.describe(&executor).await?;
    let waited = started.elapsed();
    let records = agent.records(&executor).await?;

    assert_eq!(after_start, live);
    assert!(waited >= limit, "the start waited {waited:?}");
    assert!(waited < limit * 5, "the start waited {waited:?}");
    assert!(!records.is_confirmed(&named), "{records:?}");
    assert_eq!(store.restored_names().len(), restores);
    Ok(())
}

#[test]
#[timeout("2m")]
async fn managed_snapshots_on_unmanaged_storage_fail_at_startup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let key: Box<str> = "00".repeat(64).into_boxed_str();

    let started = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_snapshots = FilesystemSnapshotsConfig::Managed(Box::new(
                    FilesystemSnapshotStoreConfig::new(&key, Duration::from_secs(60), 1, 1)
                        .expect("valid store settings"),
                ));
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await;

    match started {
        Ok(_) => Err(anyhow!(
            "the executor started with managed snapshots on unmanaged storage"
        )),
        Err(error) => {
            let message = format!("{error:#}");
            assert!(
                message.contains("filesystem snapshots require managed XFS storage"),
                "{message}"
            );
            Ok(())
        }
    }
}

/// The tree and the operation count that a run of `operations` gives on an executor without
/// snapshots.
async fn tree_without_snapshots(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    component: &PrecompiledComponent,
    operations: &[Operation],
) -> anyhow::Result<(Vec<String>, u32)> {
    let context = TestContext::new(last_unique_id);
    let root = tempfile::tempdir()?;
    let executor = start_replaying(deps, &context, root.path()).await?;
    let agent = Agent::start(&executor, &context, component, "no-snapshots", &[]).await?;
    agent.apply_all(&executor, operations).await?;
    let tree = agent.describe(&executor).await?;
    let applied = agent.applied(&executor).await?;
    executor.release().await?;
    Ok((tree, applied))
}

#[test]
#[timeout("4m")]
async fn a_newer_and_an_older_record_that_both_fail_to_load_end_in_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let operations = [
        Operation::Write {
            path: "a.txt",
            content: "a",
        },
        Operation::Write {
            path: "b.txt",
            content: "b",
        },
    ];
    let expected =
        tree_without_snapshots(deps, last_unique_id, initial_file_system, &operations).await?;
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "both-fail", &[]).await?;
    let (older, older_name) = agent.apply_and_confirm(&executor, operations[0]).await?;
    let (newer, newer_name) = agent.apply_and_confirm(&executor, operations[1]).await?;
    executor
        .return_empty_snapshot_payload(&agent.worker_id, newer)
        .await?;
    executor
        .return_empty_snapshot_payload(&agent.worker_id, older)
        .await?;
    let restores = store.restored_names().len();

    let (_, tree) = agent.restart_and_describe(&executor, &context).await?;
    let applied = agent.applied(&executor).await?;

    assert_eq!(store.restored_names()[restores..], [newer_name, older_name]);
    assert_eq!((tree, applied), expected);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_record_that_reuses_a_name_and_fails_to_load_falls_back_to_the_record_before_it(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let operations = [Operation::Write {
        path: "a.txt",
        content: "a",
    }];
    let expected =
        tree_without_snapshots(deps, last_unique_id, initial_file_system, &operations).await?;
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "reused-fails",
        &[],
    )
    .await?;
    let (first, name) = agent.apply_and_confirm(&executor, operations[0]).await?;
    // The next invocation changes no file, so its record reuses the name of the first one.
    agent.applied(&executor).await?;
    let reused = eventually(Duration::from_secs(30), || async {
        Ok(executor
            .get_oplog(&agent.worker_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Snapshot(snapshot)
                    if snapshot.filesystem_snapshot.as_deref() == Some(name.as_str())
                        && entry.oplog_index != first =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            }))
    })
    .await?;
    executor
        .return_empty_snapshot_payload(&agent.worker_id, reused)
        .await?;
    let restores = store.restored_names().len();

    let (_, tree) = agent.restart_and_describe(&executor, &context).await?;
    let applied = agent.applied(&executor).await?;

    // The start restores the name for the reused record, fails to load its payload, and restores
    // the same name again for the record before it.
    assert_eq!(store.restored_names()[restores..], [name.clone(), name]);
    assert_eq!((tree, applied), expected);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_start_attempt_from_a_record_that_fails_to_load_writes_nothing_to_the_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let failing = Agent::start(&executor, &context, initial_file_system, "failing", &[]).await?;
    let clean = Agent {
        worker_id: executor
            .start_agent(&failing.component.id, agent_id!(AGENT_TYPE, "clean"))
            .await?,
        agent: agent_id!(AGENT_TYPE, "clean"),
        component: failing.component.clone(),
    };
    let operations = [
        Operation::Write {
            path: "a.txt",
            content: "a",
        },
        Operation::Write {
            path: "b.txt",
            content: "b",
        },
    ];
    failing.apply_and_confirm(&executor, operations[0]).await?;
    let (newer, _) = failing.apply_and_confirm(&executor, operations[1]).await?;
    clean.apply_and_confirm(&executor, operations[0]).await?;
    clean.apply_and_confirm(&executor, operations[1]).await?;
    executor
        .return_empty_snapshot_payload(&failing.worker_id, newer)
        .await?;

    let (failing_kinds, failing_tree) = failing.restart_and_describe(&executor, &context).await?;
    let (clean_kinds, clean_tree) = clean.restart_and_describe(&executor, &context).await?;

    assert_eq!(failing_kinds, clean_kinds);
    assert_eq!(failing_tree, clean_tree);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_rejected_record_stays_rejected_after_a_restart_and_the_older_record_stays_selectable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let operations = [
        Operation::Write {
            path: "a.txt",
            content: "a",
        },
        Operation::Write {
            path: "b.txt",
            content: "b",
        },
    ];
    let expected =
        tree_without_snapshots(deps, last_unique_id, initial_file_system, &operations).await?;
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "rejected", &[]).await?;
    let (_, older_name) = agent.apply_and_confirm(&executor, operations[0]).await?;
    let (newer, newer_name) = agent.apply_and_confirm(&executor, operations[1]).await?;
    executor
        .return_empty_snapshot_payload(&agent.worker_id, newer)
        .await?;
    agent.stop(&executor, &context).await?;
    let restores = store.restored_names().len();

    // The start runs no invocation, so it writes no newer snapshot record.
    executor.resume(&agent.worker_id, false).await?;
    let first_start = eventually(Duration::from_secs(30), || async {
        let restored = store.restored_names()[restores..].to_vec();
        Ok((restored.len() >= 2).then_some(restored))
    })
    .await?;
    agent.stop(&executor, &context).await?;
    executor.release().await?;

    let restarted =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let restores = store.restored_names().len();
    let tree = agent.describe(&restarted).await?;
    let applied = agent.applied(&restarted).await?;
    let second_start = store.restored_names()[restores..].to_vec();

    assert_eq!(first_start, [newer_name, older_name.clone()]);
    assert_eq!(second_start, [older_name]);
    assert_eq!((tree, applied), expected);
    Ok(())
}

/// One step of a generated history of an agent.
#[derive(Clone, Copy, Debug)]
enum Step {
    Operation(Operation),
    /// A manual update to a new revision with the declaration set of this index.
    ManualUpdate(usize),
}

/// The declaration sets of the generated histories.
fn declaration_set(index: usize) -> Vec<IFSEntry> {
    match index {
        0 => vec![],
        1 => vec![entry("foo.txt", "/ro1.txt", AgentFilePermissions::ReadOnly)],
        2 => vec![
            entry("foo.txt", "/ro1.txt", AgentFilePermissions::ReadOnly),
            entry("bar.txt", "/d/ro2.txt", AgentFilePermissions::ReadOnly),
        ],
        _ => vec![entry("baz.txt", "/rw.txt", AgentFilePermissions::ReadWrite)],
    }
}

fn step_strategy() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let path = prop::sample::select(vec![
        "x.txt",
        "d",
        "d/y.txt",
        "ro1.txt",
        "rw.txt",
        "d/ro2.txt",
    ]);
    let other = prop::sample::select(vec!["x.txt", "z.txt", "d/y.txt", "e"]);
    prop_oneof![
        4 => path
            .clone()
            .prop_map(|path| Step::Operation(Operation::Write { path, content: "v" })),
        2 => path
            .clone()
            .prop_map(|path| Step::Operation(Operation::Remove { path })),
        1 => Just(Step::Operation(Operation::Mkdir { path: "d" })),
        2 => (path.clone(), other.clone())
            .prop_map(|(from, to)| Step::Operation(Operation::Rename { from, to })),
        2 => (path, other)
            .prop_map(|(existing, link)| Step::Operation(Operation::Link { existing, link })),
        1 => (0usize..4).prop_map(Step::ManualUpdate),
    ]
}

/// The tree of an agent and the number of operations that it applied.
type Outcome = (Box<[Box<str>]>, u32);

impl Agent {
    /// The tree of the agent and the number of operations that it applied.
    async fn outcome(&self, executor: &TestWorkerExecutor) -> anyhow::Result<Outcome> {
        Ok((
            self.describe(executor)
                .await?
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
            self.applied(executor).await?,
        ))
    }

    /// Runs `steps` in order. Gives the agent with the component of its last update, and the
    /// result of each step: the result of an operation, or the outcome of an update.
    async fn run_steps(
        self,
        executor: &TestWorkerExecutor,
        steps: &[Step],
    ) -> anyhow::Result<(Agent, Vec<String>)> {
        futures::stream::iter(steps)
            .map(Ok::<_, anyhow::Error>)
            .try_fold(
                (self, Vec::new()),
                |(agent, mut results), step| async move {
                    match step {
                        Step::Operation(operation) => {
                            results.push(agent.apply(executor, *operation).await?);
                            Ok((agent, results))
                        }
                        Step::ManualUpdate(set) => {
                            let updated =
                                agent.manual_update(executor, declaration_set(*set)).await?;
                            let outcome = agent
                                .update_results(executor)
                                .await?
                                .pop()
                                .ok_or_else(|| anyhow!("the update has no outcome"))?;
                            results.push(outcome);
                            Ok((
                                Agent {
                                    component: updated,
                                    ..agent
                                },
                                results,
                            ))
                        }
                    }
                },
            )
            .await
    }
}

/// How an agent of a generated history restarts.
#[derive(Clone, Copy, Debug)]
enum Restart {
    /// From its last usable periodic snapshot.
    FromSnapshot,
    /// With a full replay: before the restart, each periodic record of the agent is rejected,
    /// as a start that could not load it does, and the agent loses its periodic snapshots. The
    /// executor restarts on an empty root without periodic snapshots. A record without a
    /// filesystem snapshot name is usable without the store, so only the rejection keeps the
    /// start from selecting it. The agent stops before the rejection. The executor writes the
    /// snapshot record of an invocation after the caller gets the result, and a stopped agent
    /// writes no more records. So the rejection covers each automatic snapshot record that a
    /// start can select.
    FullReplay,
}

/// What the restart of a run of a generated history used.
#[derive(Debug)]
struct RestartSelection {
    /// The names of the restores of the restart that gave a tree.
    restored: Box<[Box<str>]>,
    /// The number of reads of automatic snapshot entries up to the end of the first call after
    /// the restart. A start reads the entry that it selected, to load its guest snapshot.
    automatic_reads: usize,
}

/// What a run of a generated history on one agent gives.
#[derive(Debug, PartialEq, Eq)]
struct HistoryRun {
    /// The result of each step, before the restart and then after it.
    results: Box<[Box<str>]>,
    /// The tree and the operation count right after the restart.
    restarted: Outcome,
    /// The tree and the operation count after the steps that follow the restart.
    finished: Outcome,
}

/// Runs `before` on an agent with snapshots, restarts it as `restart` says, and runs `after`
/// live. Gives the run and what the restart used.
async fn run_history(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    component: &PrecompiledComponent,
    initial: usize,
    (before, after): (&[Step], &[Step]),
    restart: Restart,
) -> anyhow::Result<(HistoryRun, RestartSelection)> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        component,
        "history",
        &declaration_set(initial),
    )
    .await?;
    let (agent, mut results) = agent.run_steps(&executor, before).await?;
    if let Restart::FullReplay = restart {
        agent.stop(&executor, &context).await?;
        let periodic_records = executor
            .get_oplog(&agent.worker_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>();
        executor
            .reject_automatic_snapshots(&agent.worker_id, periodic_records)
            .await?;
    }
    let (owned, fingerprint) = agent.incarnation(&executor, &context).await?;
    executor.release().await?;
    let root = tempfile::tempdir()?;
    if let Restart::FullReplay = restart {
        futures::stream::iter(store.snapshot_names(&owned, fingerprint).await)
            .filter(|name| std::future::ready(name.starts_with("p-")))
            .for_each(|name| {
                let (store, owned) = (&store, &owned);
                async move { store.lose(owned, fingerprint, &name).await }
            })
            .await;
    }
    let restores = store.completed_restore_names().len();
    let restarted = match restart {
        Restart::FromSnapshot => {
            start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?
        }
        Restart::FullReplay => {
            start_replaying_with(deps, &context, root.path(), Some(&store)).await?
        }
    };
    let restarted_outcome = agent.outcome(&restarted).await?;
    let selection = RestartSelection {
        restored: store.completed_restore_names()[restores..]
            .iter()
            .map(|name| name.as_str().into())
            .collect(),
        automatic_reads: restarted
            .oplog_service_call_count(&agent.worker_id, "read_automatic_snapshot"),
    };
    let (agent, after_results) = agent.run_steps(&restarted, after).await?;
    results.extend(after_results);
    let finished = agent.outcome(&restarted).await?;
    restarted.release().await?;
    Ok((
        HistoryRun {
            results: results.into_iter().map(String::into_boxed_str).collect(),
            restarted: restarted_outcome,
            finished,
        },
        selection,
    ))
}

#[test]
#[timeout("15m")]
async fn generated_histories_restart_from_a_snapshot_with_the_tree_and_the_results_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use proptest::strategy::{Strategy, ValueTree};
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let histories = (
        0usize..3,
        proptest::collection::vec(step_strategy(), 1..12),
        0usize..12,
    );
    let cases = std::iter::repeat_with(|| {
        histories
            .new_tree(&mut runner)
            .map(|tree| tree.current())
            .map_err(|error| anyhow!("{error}"))
    })
    .take(10)
    .collect::<anyhow::Result<Vec<_>>>()?;

    let outcomes = futures::stream::iter(cases)
        .then(|(initial, steps, restart_at)| async move {
            let (before, after) = steps.split_at(restart_at.min(steps.len()));
            let run = |restart| {
                run_history(
                    deps,
                    last_unique_id,
                    initial_file_system,
                    initial,
                    (before, after),
                    restart,
                )
            };
            let (from_snapshot, snapshot_selection) = run(Restart::FromSnapshot).await?;
            let (full_replay, replay_selection) = run(Restart::FullReplay).await?;
            Ok::<_, anyhow::Error>((
                (from_snapshot != full_replay).then(|| {
                    format!(
                        "{initial} {before:?} | {after:?}: from a snapshot {from_snapshot:#?}, \
                         with a full replay {full_replay:#?}"
                    )
                }),
                snapshot_selection,
                (format!("{initial} {before:?}"), replay_selection),
            ))
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;
    let failures = outcomes
        .iter()
        .filter_map(|(failure, _, _)| failure.clone())
        .collect::<Vec<_>>();
    let snapshot_restores = outcomes
        .iter()
        .map(|(_, selection, _)| selection.restored.len())
        .sum::<usize>();
    let replays_from_a_periodic_record = outcomes
        .iter()
        .filter(|(_, _, (_, selection))| {
            selection.automatic_reads > 0
                || selection.restored.iter().any(|name| name.starts_with("p-"))
        })
        .map(|(_, _, (case, selection))| format!("{case}: {selection:?}"))
        .collect::<Vec<_>>();

    assert!(failures.is_empty(), "{failures:#?}");
    assert!(snapshot_restores > 0, "no restart restored a snapshot");
    assert!(
        replays_from_a_periodic_record.is_empty(),
        "a full-replay restart used a periodic record: {replays_from_a_periodic_record:#?}"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn managed_xfs_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

/// The store settings of the managed XFS tests: the store over the blob storage of the executor.
#[cfg(target_os = "linux")]
fn managed_snapshots() -> FilesystemSnapshotsConfig {
    FilesystemSnapshotsConfig::Managed(Box::new(
        FilesystemSnapshotStoreConfig::new(&"5a".repeat(64), Duration::from_secs(60), 2, 2)
            .expect("valid store settings")
            .with_uploads(uploads(Duration::from_secs(30))),
    ))
}

#[cfg(target_os = "linux")]
const MANAGED_XFS_DISK_SPACE: u64 = 64 * 1024 * 1024;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("4m")]
async fn managed_xfs_restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_managed_xfs;

    let context = TestContext::new(last_unique_id);
    let start = |snapshots| {
        start_with_filesystem_snapshots_on_managed_xfs(
            deps,
            &context,
            MANAGED_XFS_DISK_SPACE,
            managed_xfs_test_root(),
            snapshots,
        )
    };
    let executor = start(managed_snapshots()).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "managed-xfs-replay",
        &[
            entry("foo.txt", "/ro-renamed.txt", AgentFilePermissions::ReadOnly),
            entry(
                "bar.txt",
                "/dir/ro-in-dir.txt",
                AgentFilePermissions::ReadOnly,
            ),
            entry(
                "baz.txt",
                "/rw-modified.txt",
                AgentFilePermissions::ReadWrite,
            ),
        ],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Write {
                    path: "rw-modified.txt",
                    content: "modified by the agent",
                },
                Operation::Rename {
                    from: "ro-renamed.txt",
                    to: "renamed.txt",
                },
                Operation::Rename {
                    from: "dir",
                    to: "moved-dir",
                },
                Operation::Write {
                    path: "agent.txt",
                    content: "agent data",
                },
                Operation::Link {
                    existing: "agent.txt",
                    link: "agent-alias.txt",
                },
                Operation::Symlink {
                    link: "pointer",
                    target: "agent.txt",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let live = agent.describe(&executor).await?;
    executor.release().await?;

    let restarted = start(managed_snapshots()).await?;
    let restored = agent.describe(&restarted).await?;
    restarted.release().await?;
    let replaying = start(FilesystemSnapshotsConfig::default()).await?;
    let replayed = agent.describe(&replaying).await?;

    assert_eq!(restored, live);
    assert_eq!(replayed, live);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("4m")]
async fn managed_xfs_manual_update_brings_the_files_into_the_target_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_managed_xfs;

    let context = TestContext::new(last_unique_id);
    let start = || {
        start_with_filesystem_snapshots_on_managed_xfs(
            deps,
            &context,
            MANAGED_XFS_DISK_SPACE,
            managed_xfs_test_root(),
            managed_snapshots(),
        )
    };
    let executor = start().await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "managed-xfs-manual-update",
        &[entry(
            "foo.txt",
            "/ro-old.txt",
            AgentFilePermissions::ReadOnly,
        )],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Mkdir { path: "work" },
                Operation::Write {
                    path: "work/state.db",
                    content: "rows",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![entry(
                "bar.txt",
                "/ro-new.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    let updated_agent = Agent {
        component: updated,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let after_update = updated_agent.describe(&executor).await?;
    executor.release().await?;
    let restarted = start().await?;

    assert_eq!(
        after_update,
        [
            r#"ro-new.txt file links=1 writable=false content="bar\n""#,
            "work dir",
            r#"work/state.db file links=1 writable=true content="rows""#,
        ]
        .map(String::from)
    );
    assert_eq!(updated_agent.describe(&restarted).await?, after_update);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_manual_update_during_an_upload_waits_for_the_upload_and_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "update-waits",
        &[],
    )
    .await?;
    store.set_save_delay(Duration::from_secs(2));
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "state.db",
                content: "rows",
            }],
        )
        .await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.oldest_name())
    })
    .await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![],
        )
        .await?;

    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    store.set_save_delay(Duration::ZERO);
    let updated_agent = Agent {
        component: updated,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let tree = updated_agent.describe(&executor).await?;
    let records = updated_agent.records(&executor).await?;

    assert!(records.failed_updates.is_empty(), "{records:?}");
    assert_eq!(
        tree,
        [r#"state.db file links=1 writable=true content="rows""#].map(String::from)
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_newest_record_whose_filesystem_does_not_restore_falls_back_to_the_older_record(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let operations = [
        Operation::Write {
            path: "a.txt",
            content: "a",
        },
        Operation::Write {
            path: "b.txt",
            content: "b",
        },
    ];
    let expected =
        tree_without_snapshots(deps, last_unique_id, initial_file_system, &operations).await?;
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "restore-fails",
        &[],
    )
    .await?;
    let (_, older_name) = agent.apply_and_confirm(&executor, operations[0]).await?;
    let (_, newer_name) = agent.apply_and_confirm(&executor, operations[1]).await?;
    store.fail_restores_of(&newer_name);
    agent.stop(&executor, &context).await?;
    let restores = store.restored_names().len();

    let tree = agent.describe(&executor).await?;
    let applied = agent.applied(&executor).await?;

    assert_eq!(store.restored_names()[restores..], [newer_name, older_name]);
    assert_eq!((tree, applied), expected);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_start_during_an_upload_waits_for_it_and_then_confirms_its_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(&executor, &context, initial_file_system, "start-waits", &[]).await?;
    store.set_save_delay(Duration::from_secs(3));
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "file.txt",
                content: "content",
            }],
        )
        .await?;
    let live = agent.describe(&executor).await?;
    let named = eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.newest_name())
    })
    .await?;
    let (owned, fingerprint) = agent.incarnation(&executor, &context).await?;
    agent.stop(&executor, &context).await?;
    let stored_at_start = store
        .snapshot_names(&owned, fingerprint)
        .await
        .contains(&named);
    let restores = store.restored_names().len();

    let started = std::time::Instant::now();
    executor.resume(&agent.worker_id, false).await?;
    let after_start = agent.describe(&executor).await?;
    let waited = started.elapsed();
    let records = agent.records(&executor).await?;
    store.set_save_delay(Duration::ZERO);

    assert!(!stored_at_start);
    assert!(records.is_confirmed(&named), "{records:?}");
    assert_eq!(store.restored_names()[restores..], [named]);
    assert_eq!(after_start, live);
    assert!(
        waited < Duration::from_secs(20),
        "the start waited {waited:?}"
    );
    Ok(())
}

/// Runs `future` and gives its output with the time it took.
async fn timed<T>(future: impl std::future::Future<Output = T>) -> (T, Duration) {
    let started = std::time::Instant::now();
    let output = future.await;
    (output, started.elapsed())
}

/// An invocation that arrives during the stop is not held back by the stop path; its start waits
/// for the upload for at most `confirmation_wait`.
#[test]
#[timeout("4m")]
async fn an_invocation_during_a_stop_waits_for_the_upload_only_in_its_start_and_at_most_the_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let upload = Duration::from_secs(60);
    let confirmation_wait = Duration::from_secs(2);
    let margin = Duration::from_secs(5);
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor = start_snapshotting(deps, &context, &store, confirmation_wait, None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "stop-no-wait",
        &[],
    )
    .await?;
    store.set_save_delay(upload);
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "file.txt",
                content: "content",
            }],
        )
        .await?;
    let live = agent.describe(&executor).await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.oldest_name())
    })
    .await?;

    let ((stopped, stop_took), (tree, invocation_took)) = futures::join!(
        timed(agent.stop(&executor, &context)),
        timed(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            agent.describe(&executor).await
        })
    );

    stopped?;
    assert_eq!(tree?, live);
    assert!(stop_took < confirmation_wait, "the stop took {stop_took:?}");
    assert!(
        invocation_took < confirmation_wait + margin,
        "the invocation took {invocation_took:?}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_named_manual_update_record_fails_a_start_without_filesystem_snapshots_visibly(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "disabled-start",
        &[],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "state.db",
                content: "rows",
            }],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    executor.release().await?;

    let root = tempfile::tempdir()?;
    let disabled = start_replaying(deps, &context, root.path()).await?;
    let started = agent.describe(&disabled).await;
    let metadata = disabled.get_worker_metadata(&agent.worker_id).await?;

    let error = format!("{:?}", started.err());
    assert!(
        error.contains("filesystem snapshots are disabled on this executor"),
        "{error}"
    );
    assert!(
        metadata
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("filesystem snapshots are disabled")),
        "{metadata:?}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn failed_manual_updates_never_delete_the_snapshot_of_the_last_successful_one(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let spacing = Duration::from_secs(10 * 60);
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let declared = || entry("baz.txt", "/config.txt", AgentFilePermissions::ReadWrite);
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "update-baseline",
        &[declared()],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Write {
                    path: "config.txt",
                    content: "changed",
                },
                Operation::Write {
                    path: "state.db",
                    content: "rows",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let (owned, fingerprint) = agent.incarnation(&executor, &context).await?;
    let update_names = || async {
        store
            .snapshot_names(&owned, fingerprint)
            .await
            .into_iter()
            .filter(|name| name.starts_with("u-"))
            .collect::<Vec<_>>()
    };

    store.advance_clock(spacing);
    let successful = agent.manual_update(&executor, vec![declared()]).await?;
    let baseline = update_names().await;
    let failing = || entry("foo.txt", "/config.txt", AgentFilePermissions::ReadWrite);
    let failed = futures::stream::iter(0..3)
        .then(|_| {
            let store = &store;
            let agent = &agent;
            let executor = &executor;
            async move {
                store.advance_clock(spacing);
                agent.manual_update(executor, vec![failing()]).await
            }
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;
    let after_retention = eventually(Duration::from_secs(30), || async {
        let names = update_names().await;
        Ok((names.len() == baseline.len() + 2).then_some(names))
    })
    .await?;
    let records = agent.records(&executor).await?;
    futures::stream::iter(store.snapshot_names(&owned, fingerprint).await)
        .filter(|name| std::future::ready(name.starts_with("p-")))
        .for_each(|name| {
            let (store, owned) = (&store, &owned);
            async move { store.lose(owned, fingerprint, &name).await }
        })
        .await;
    agent.stop(&executor, &context).await?;
    let restores = store.restored_names().len();
    let current = Agent {
        component: successful,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let tree = current.describe(&executor).await;

    assert_eq!(baseline.len(), 1);
    assert_eq!(failed.len(), 3);
    assert_eq!(records.failed_updates.len(), 3, "{records:?}");
    assert!(
        after_retention.contains(&baseline[0]),
        "{after_retention:?}"
    );
    assert_eq!(store.restored_names()[restores..], baseline[..]);
    assert_eq!(
        tree?,
        [
            r#"config.txt file links=1 writable=true content="changed""#,
            r#"state.db file links=1 writable=true content="rows""#,
        ]
        .map(String::from)
    );
    Ok(())
}

/// Asks for a manual update of `agent` while `store` holds each save for a minute, interrupts
/// the agent, and gives the details of the failed update with the time from the interrupt to it.
async fn interrupt_a_held_manual_update(
    executor: &TestWorkerExecutor,
    store: &TestFilesystemSnapshotStore,
    agent: &Agent,
) -> anyhow::Result<(Vec<String>, Duration)> {
    store.set_save_delay(Duration::from_secs(60));
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (failed, took) = timed(async {
        executor
            .interrupt_loaded_worker(
                &agent.worker_id,
                golem_service_base::error::worker_executor::InterruptKind::Interrupt(
                    golem_common::model::Timestamp::now_utc(),
                ),
            )
            .await?;
        agent.failed_updates(executor).await
    })
    .await;
    store.set_save_delay(Duration::ZERO);
    Ok((failed?, took))
}

#[test]
#[timeout("4m")]
async fn a_terminal_interrupt_ends_a_manual_update_that_waits_for_an_upload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "interrupted-wait",
        &[],
    )
    .await?;
    store.set_save_delay(Duration::from_secs(60));
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "state.db",
                content: "rows",
            }],
        )
        .await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.oldest_name())
    })
    .await?;

    let (failed, took) = interrupt_a_held_manual_update(&executor, &store, &agent).await?;

    assert!(
        failed
            .iter()
            .any(|failure| failure.contains("interrupted while it waited")),
        "{failed:?}"
    );
    assert!(
        took < Duration::from_secs(10),
        "the update ended after {took:?}"
    );
    Ok(())
}

/// The target revision of a manual-update invocation entry.
fn manual_update_target(entry: &OplogEntry) -> Option<ComponentRevision> {
    let OplogEntry::PendingAgentInvocation { payload, .. } = entry else {
        return None;
    };
    let payload = match payload {
        OplogPayload::Inline(payload) => (**payload).clone(),
        OplogPayload::SerializedInline { bytes, .. } => {
            golem_common::serialization::deserialize::<AgentInvocationPayload>(bytes).ok()?
        }
        OplogPayload::External { .. } => return None,
    };
    match payload {
        AgentInvocationPayload::ManualUpdate { target_revision } => Some(target_revision),
        _ => None,
    }
}

/// The incarnation of the shard manager that the test executors trust.
const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000001";

/// Revokes shard 0, the only shard of a test executor. The executor retires its agents for a
/// lost shard.
async fn revoke_shard_zero(executor: &TestWorkerExecutor) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::shardmanager::ShardId;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        RevokeShardsRequest, revoke_shards_response,
    };

    let revoked = executor
        .client
        .clone()
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![ShardId { value: 0 }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    match revoked.result {
        Some(revoke_shards_response::Result::Success(_)) => Ok(()),
        other => Err(anyhow!("the executor refused the revoke: {other:?}")),
    }
}

#[test]
#[timeout("4m")]
async fn a_lost_shard_ends_a_manual_update_that_waits_for_an_upload_without_a_failed_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "lost-shard-wait",
        &[],
    )
    .await?;
    store.set_save_delay(Duration::from_secs(60));
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "state.db",
                content: "rows",
            }],
        )
        .await?;
    eventually(Duration::from_secs(30), || async {
        let records = agent.records(&executor).await?;
        Ok(records.oldest_name())
    })
    .await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    revoke_shard_zero(&executor).await?;
    let owned = agent.owned(&context);
    eventually(Duration::from_secs(30), || async {
        Ok((!executor.worker_is_loaded(&owned).await).then_some(()))
    })
    .await?;
    store.set_save_delay(Duration::ZERO);
    let oplog = executor.stored_oplog(&agent.worker_id).await;

    let target = updated.revision;
    let requested = oplog
        .iter()
        .filter(|entry| manual_update_target(entry) == Some(target))
        .count();
    let consumed = oplog
        .iter()
        .filter(|entry| match entry {
            OplogEntry::FailedUpdate {
                target_revision, ..
            }
            | OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } => *target_revision == target,
            OplogEntry::PendingUpdate { description, .. } => {
                description.target_revision() == &target
            }
            _ => false,
        })
        .count();
    assert_eq!(
        (requested, consumed),
        (1, 0),
        "the manual update must stay pending: {oplog:?}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_terminal_interrupt_ends_a_manual_update_during_its_upload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "interrupted-upload",
        &[],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[Operation::Write {
                path: "state.db",
                content: "rows",
            }],
        )
        .await?;
    agent.confirmed(&executor).await?;

    let trees = store.saved_trees().len();
    let (failed, took) = interrupt_a_held_manual_update(&executor, &store, &agent).await?;
    let capture = store.saved_trees()[trees..].to_vec();

    assert!(
        failed
            .iter()
            .any(|failure| failure.contains("interrupted while it uploaded")),
        "{failed:?}"
    );
    assert_eq!(capture.len(), 1);
    assert!(!capture[0].exists(), "the capture {capture:?} stays");
    assert!(
        took < Duration::from_secs(10),
        "the update ended after {took:?}"
    );
    Ok(())
}

/// Waits until the oplog of `worker_id` holds, after its last `AgentInvocationFinished`, a
/// snapshot record with a name, and a confirmation of that name. The caller calls it after an
/// invocation that ends at a snapshot boundary, so the record is the snapshot of that invocation,
/// and no snapshot record is waiting for its upload when the wait ends.
async fn newest_snapshot_confirmed(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<()> {
    eventually(Duration::from_secs(30), || async {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let after_last_invocation = oplog
            .iter()
            .rposition(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .map_or(&oplog[..], |last| &oplog[last..]);
        let newest = after_last_invocation
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Snapshot(snapshot) => snapshot.filesystem_snapshot.clone(),
                _ => None,
            });
        Ok(newest.filter(|name| {
            oplog.iter().any(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::SnapshotConfirmed(confirmed)
                        if &confirmed.filesystem_snapshot == name
                )
            })
        }))
    })
    .await
    .map(|_| ())
}

/// The state that `getState` of `SqliteSnapshotAgent` gives.
async fn sqlite_state(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentId,
) -> anyhow::Result<serde_json::Value> {
    let state = executor
        .invoke_and_await_agent(component, agent, "getState", data_value!())
        .await?
        .into_typed::<String>()?;
    Ok(serde_json::from_str(&state)?)
}

/// `SqliteSnapshotAgent` holds an in-memory database, a file-backed database at an absolute path
/// and a file-backed database at a relative path. The snapshot holds the bytes of the in-memory
/// database and the locations of the file-backed ones. After a restart the filesystem snapshot
/// gives back the database files, the typed load opens them again at their locations, and the
/// agent has all its rows.
#[test]
#[timeout("4m")]
async fn ts_sqlite_snapshot_keeps_in_memory_databases_and_restores_file_databases_from_the_filesystem_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo")] constructor_parameter_echo: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let component = executor
        .component_dep(&context.default_environment_id, constructor_parameter_echo)
        .store()
        .await?;
    let agent = agent_id!("SqliteSnapshotAgent", "sqlite-recovery");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(&component, &agent, "addItem", data_value!("apple"))
        .await?;
    newest_snapshot_confirmed(&executor, &worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "addItem", data_value!("banana"))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "addLog", data_value!("started"))
        .await?;
    newest_snapshot_confirmed(&executor, &worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "setLabel", data_value!("after-init"))
        .await?;
    let before = sqlite_state(&executor, &component, &agent).await?;
    assert_eq!(
        before,
        serde_json::json!({
            "label": "after-init",
            "items": ["apple", "banana"],
            "logs": ["started"],
            "notes": ["started"],
        })
    );
    newest_snapshot_confirmed(&executor, &worker_id).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let multipart = oplog
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(snapshot) => Some(snapshot.data.clone()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("no snapshot record before the restart"))?;
    let PublicSnapshotData::Multipart(multipart) = multipart else {
        return Err(anyhow!("the snapshot is not multipart: {multipart:?}"));
    };
    let names: Vec<&str> = multipart.parts.iter().map(|part| &*part.name).collect();
    assert_eq!(names, vec!["state", "db:memDb"]);
    assert_eq!(multipart.parts[1].content_type, "application/x-sqlite3");
    let MultipartPartData::Raw(memory_database) = &multipart.parts[1].data else {
        return Err(anyhow!("the db:memDb part is not raw bytes"));
    };
    assert!(
        !memory_database.data.is_empty(),
        "the db:memDb part is empty"
    );
    let MultipartPartData::Json(envelope) = &multipart.parts[0].data else {
        return Err(anyhow!("the state part is not JSON"));
    };
    let file_databases = envelope
        .data
        .get("fileDatabases")
        .ok_or_else(|| anyhow!("the envelope has no fileDatabases: {:?}", envelope.data))?;
    assert_eq!(
        file_databases["fileDb"],
        serde_json::json!("/tmp/sqlite-snapshot-test.db")
    );
    let relative = file_databases["relativeDb"]
        .as_str()
        .ok_or_else(|| anyhow!("no location for relativeDb: {file_databases:?}"))?;
    assert!(
        relative.starts_with('/') && relative.ends_with("/sqlite-relative-test.db"),
        "the location of relativeDb is {relative}"
    );

    executor.release().await?;
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let after = sqlite_state(&executor, &component, &agent).await?;
    assert_snapshot_recovery_loaded(&mut events).await;
    assert_eq!(after, before);

    executor
        .invoke_and_await_agent(&component, &agent, "addItem", data_value!("cherry"))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "addLog", data_value!("recovered"))
        .await?;
    assert_eq!(
        sqlite_state(&executor, &component, &agent).await?,
        serde_json::json!({
            "label": "after-init",
            "items": ["apple", "banana", "cherry"],
            "logs": ["started", "recovered"],
            "notes": ["started", "recovered"],
        })
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        invocation_shape(&executor.stored_oplog(&worker_id).await),
        InvocationShape::settled()
    );
    Ok(())
}

/// `SqliteSnapshotAgent` takes a snapshot after every second invocation (the constructor counts
/// as one), so the `getState` below is recorded after the last snapshot and is replayed after the
/// restart. The typed load opens the file-backed databases again on the files that the filesystem
/// snapshot gave back, and the replayed `getState` must make the same filesystem host calls as
/// the live one did, so the snapshot recovery succeeds without a fall back to a full replay.
#[test]
#[timeout("4m")]
async fn ts_sqlite_tail_replay_after_a_filesystem_restore_matches_the_live_run(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo")] constructor_parameter_echo: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = TestFilesystemSnapshotStore::new();
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let component = executor
        .component_dep(&context.default_environment_id, constructor_parameter_echo)
        .store()
        .await?;
    let agent = agent_id!("SqliteSnapshotAgent", "sqlite-tail-replay");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(&component, &agent, "addItem", data_value!("apple"))
        .await?;
    newest_snapshot_confirmed(&executor, &worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "addLog", data_value!("started"))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "setLabel", data_value!("after-init"))
        .await?;
    newest_snapshot_confirmed(&executor, &worker_id).await?;
    let before = sqlite_state(&executor, &component, &agent).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let last_snapshot = oplog
        .iter()
        .rposition(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
        .ok_or_else(|| anyhow!("no snapshot record before the restart"))?;
    assert!(
        oplog[last_snapshot..]
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "no invocation is recorded after the last snapshot"
    );

    executor.release().await?;
    let executor =
        start_snapshotting(deps, &context, &store, Duration::from_secs(30), None).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let after = sqlite_state(&executor, &component, &agent).await?;
    assert_snapshot_recovery_loaded(&mut events).await;
    assert_eq!(after, before);

    executor
        .invoke_and_await_agent(&component, &agent, "addLog", data_value!("recovered"))
        .await?;
    assert_eq!(
        sqlite_state(&executor, &component, &agent).await?,
        serde_json::json!({
            "label": "after-init",
            "items": ["apple"],
            "logs": ["started", "recovered"],
            "notes": ["started", "recovered"],
        })
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        invocation_shape(&executor.stored_oplog(&worker_id).await),
        InvocationShape::settled()
    );
    Ok(())
}
