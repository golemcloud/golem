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

//! Never-live forks: a marked agent replays its history on an automatic update, writes one
//! update outcome and stops. It never runs guest code live and never reaches a peer.

use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex, UpdateFailureClass};
use golem_common::model::{AgentId, AgentStatus, OplogIndex, OwnedAgentId};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v3")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// A fork target: a phantom of the source's agent type and constructor parameters.
pub(crate) struct ForkTarget {
    pub(crate) agent: ParsedAgentKey,
    pub(crate) worker_id: AgentId,
}

impl ForkTarget {
    pub(crate) fn new(component: &ComponentDto, agent: ParsedAgentKey) -> anyhow::Result<Self> {
        Ok(Self {
            worker_id: AgentId::from_agent_id(component.id, &agent).map_err(anyhow::Error::msg)?,
            agent,
        })
    }
}

/// The index of the last entry of the agent's oplog.
pub(crate) async fn tip(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<OplogIndex> {
    executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .map(|entry| entry.oplog_index)
        .ok_or_else(|| anyhow!("the oplog of {worker_id} is empty"))
}

/// The update outcomes of the agent's oplog once it holds one and the agent is unloaded.
pub(crate) async fn settled_update_outcomes(
    executor: &TestWorkerExecutor,
    environment_id: EnvironmentId,
    worker_id: &AgentId,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    let owned = OwnedAgentId::new(environment_id, worker_id);
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let outcomes = update_outcomes(executor, worker_id).await?;
            if !outcomes.is_empty() && !executor.worker_is_loaded(&owned).await {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    update_outcomes(executor, worker_id).await
}

async fn update_outcomes(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    Ok(executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.entry,
                PublicOplogEntry::SuccessfulUpdate(_) | PublicOplogEntry::FailedUpdate(_)
            )
        })
        .collect())
}

/// Entries that only a live generation appends.
fn live_entries(oplog: &[PublicOplogEntryWithIndex], after: OplogIndex) -> Vec<String> {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter(|entry| {
            matches!(
                entry.entry,
                PublicOplogEntry::AgentInvocationStarted(_)
                    | PublicOplogEntry::AgentInvocationFinished(_)
                    | PublicOplogEntry::Start(_)
                    | PublicOplogEntry::End(_)
                    | PublicOplogEntry::Error(_)
                    | PublicOplogEntry::Snapshot(_)
            )
        })
        .map(|entry| format!("{}: {:?}", entry.oplog_index, entry.entry))
        .collect()
}

/// A never-live fork refuses invocations, shows the `Sandboxed` status, refuses a manual update,
/// and replays its history on an automatic update to its own revision: one `SuccessfulUpdate`,
/// no live work, and no further start. A fork of it is never-live too. An ordinary agent still
/// refuses an update to its own revision.
#[test]
#[timeout("4m")]
async fn a_never_live_fork_replays_an_update_to_its_own_revision_and_never_goes_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let source = agent_id!("SnapshotUpdateTest");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "pre_snapshot_value", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &source, "stable_value", data_value!())
        .await?;
    let cut = tip(&executor, &source_id).await?;

    let fork = ForkTarget::new(
        &component,
        phantom_agent_id!("SnapshotUpdateTest", uuid::Uuid::new_v4()),
    )?;
    executor
        .fork_worker_never_live(&source_id, &fork.agent.to_string(), cut)
        .await?;

    let refused = executor
        .invoke_and_await_agent(&component, &fork.agent, "accumulated_value", data_value!())
        .await
        .expect_err("a never-live fork refuses an invocation");
    assert!(
        format!("{refused:#}").contains("never-live sandbox fork"),
        "unexpected error: {refused:#}"
    );
    let metadata = executor.get_worker_metadata(&fork.worker_id).await?;
    assert_eq!(metadata.status, AgentStatus::Sandboxed);
    assert!(metadata.underlying_status.is_some());

    executor
        .manual_update_worker(&fork.worker_id, component.revision, false)
        .await
        .expect_err("a never-live fork refuses a manual update");

    let before_update = tip(&executor, &fork.worker_id).await?;
    executor
        .auto_update_worker(&fork.worker_id, component.revision, false)
        .await?;
    let outcomes =
        settled_update_outcomes(&executor, context.default_environment_id, &fork.worker_id).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    let oplog = executor
        .get_oplog(&fork.worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(live_entries(&oplog, before_update), Vec::<String>::new());
    let metadata = executor.get_worker_metadata(&fork.worker_id).await?;
    assert_eq!(metadata.status, AgentStatus::Sandboxed);
    assert_eq!(metadata.component_revision, component.revision);

    let second = ForkTarget::new(
        &component,
        phantom_agent_id!("SnapshotUpdateTest", uuid::Uuid::new_v4()),
    )?;
    executor
        .fork_worker(
            &fork.worker_id,
            &second.agent.to_string(),
            tip(&executor, &fork.worker_id).await?,
        )
        .await?;
    assert_eq!(
        executor
            .get_worker_metadata(&second.worker_id)
            .await?
            .status,
        AgentStatus::Sandboxed
    );

    executor
        .auto_update_worker(&source_id, component.revision, false)
        .await
        .expect_err("an ordinary agent refuses an update to its own revision");
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &source, "accumulated_value", data_value!())
            .await?
            .into_typed::<u32>()?,
        11
    );
    Ok(())
}

/// Counts the reads of the bucket `snapshot-load`.
struct CountingKeyValueService {
    inner: Arc<dyn KeyValueService>,
    reads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl KeyValueService for CountingKeyValueService {
    async fn get(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if bucket == "snapshot-load" {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.get(environment_id, bucket, key).await
    }

    async fn delete(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()> {
        self.inner.delete(environment_id, bucket, key).await
    }

    async fn delete_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        keys: Arc<[String]>,
    ) -> anyhow::Result<()> {
        self.inner.delete_many(environment_id, bucket, keys).await
    }

    async fn exists(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        self.inner.exists(environment_id, bucket, key).await
    }

    async fn get_keys(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
    ) -> anyhow::Result<Vec<String>> {
        self.inner.get_keys(environment_id, bucket).await
    }

    async fn get_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        keys: Arc<[String]>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        self.inner.get_many(environment_id, bucket, keys).await
    }

    async fn set(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.inner
            .set(environment_id, bucket, key, outgoing_value)
            .await
    }

    async fn set_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key_values: Arc<[(String, Vec<u8>)]>,
    ) -> anyhow::Result<()> {
        self.inner
            .set_many(environment_id, bucket, key_values)
            .await
    }
}

/// A never-live fork cut after a periodic snapshot record gets an automatic update to a newer
/// revision with the same code. The record is the baseline of the update, so the target's load
/// hook runs, and it reads a key: a remote read, which read-only strictness admits in a load hook.
/// The host-call decision table refuses it before it reaches the store, and the update fails as
/// not replayable. The control, an unmarked fork of the same cut, reads the key and succeeds.
#[test]
#[timeout("4m")]
async fn a_key_value_read_in_the_load_hook_of_a_never_live_fork_is_refused_as_not_replayable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v3")] agent_update_v3: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    let executor = crate::fork::start_with_local_resume_and(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(
                |config: &mut golem_worker_executor::services::golem_config::GolemConfig| {
                    config.oplog.default_snapshotting =
                        SnapshotPolicy::EveryNInvocation { count: 1 };
                },
            )),
            wrap_key_value_service: Some(Arc::new(move |inner| {
                Arc::new(CountingKeyValueService {
                    inner,
                    reads: counted.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v3)
        .store()
        .await?;
    let source = agent_id!("KeyValueLoadTest");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "set", data_value!(42u64))
        .await?;
    let snapshot =
        crate::hot_update::wait_for_snapshot_after(&executor, &source_id, OplogIndex::INITIAL)
            .await?;
    let cut = tip(&executor, &source_id).await?;
    assert!(snapshot <= cut);
    let target = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;

    let marked = ForkTarget::new(
        &component,
        phantom_agent_id!("KeyValueLoadTest", uuid::Uuid::new_v4()),
    )?;
    executor
        .fork_worker_never_live(&source_id, &marked.agent.to_string(), cut)
        .await?;
    executor
        .auto_update_worker(&marked.worker_id, target.revision, false)
        .await?;
    let outcomes =
        settled_update_outcomes(&executor, context.default_environment_id, &marked.worker_id)
            .await?;

    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    let PublicOplogEntry::FailedUpdate(failed) = &outcomes[0].entry else {
        return Err(anyhow!("the update did not fail: {outcomes:?}"));
    };
    assert_eq!(failed.failure_class, UpdateFailureClass::NotReplayable);
    assert!(
        failed.snapshot_assisted_details.is_some(),
        "the record is not the baseline: {failed:?}"
    );
    assert!(
        failed
            .details
            .as_deref()
            .is_some_and(|details| details.contains("keyvalue")),
        "the refused function is not named: {failed:?}"
    );
    assert_eq!(
        executor
            .get_worker_metadata(&marked.worker_id)
            .await?
            .component_revision,
        component.revision
    );

    let unmarked = ForkTarget::new(
        &component,
        phantom_agent_id!("KeyValueLoadTest", uuid::Uuid::new_v4()),
    )?;
    executor
        .fork_worker(&source_id, &unmarked.agent.to_string(), cut)
        .await?;
    executor
        .auto_update_worker(&unmarked.worker_id, target.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &unmarked.worker_id,
            target.revision,
            Duration::from_secs(60),
        )
        .await?;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    Ok(())
}

/// The kinds of the entries after `after`, in oplog order.
fn entry_kinds(oplog: &[PublicOplogEntryWithIndex], after: OplogIndex) -> Vec<String> {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .map(|entry| {
            let rendered = format!("{:?}", entry.entry);
            rendered
                .split(['(', ' ', '{'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Checks that `kinds` are the entries of one automatic update: its `PendingUpdate` entries (the
/// admission and the chosen strategy), then exactly one outcome.
fn assert_only_update_entries(kinds: &[String]) {
    let (outcome, pending) = kinds.split_last().expect("no entries were appended");
    assert!(
        !pending.is_empty() && pending.iter().all(|kind| kind == "PendingUpdate"),
        "{kinds:?}"
    );
    assert!(
        outcome == "SuccessfulUpdate" || outcome == "FailedUpdate",
        "{kinds:?}"
    );
}

/// A never-live fork of an agent that was interrupted during an invocation keeps the interrupted
/// invocation as it is: each automatic update appends only its `PendingUpdate` entries and its
/// outcome, and no `Resumed`, also after a cold restart of the executor.
#[test]
#[timeout("4m")]
async fn an_interrupted_never_live_fork_appends_only_its_update_outcomes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // The parked wait must not suspend, so that the interrupt finds a running invocation.
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let source = agent_id!("Clock", "never-live-interrupted");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "healthcheck", data_value!())
        .await?;
    let sleeping = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let source = source.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &source, "sleep_p3", data_value!(60u64))
                .await
        }
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    executor.interrupt(&source_id).await?;
    assert!(sleeping.await?.is_err());
    executor
        .wait_for_status(
            &source_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let cut = tip(&executor, &source_id).await?;

    let fork = ForkTarget::new(
        &component,
        phantom_agent_id!("Clock", uuid::Uuid::new_v4(), "never-live-interrupted"),
    )?;
    executor
        .fork_worker_never_live(&source_id, &fork.agent.to_string(), cut)
        .await?;
    let forked = tip(&executor, &fork.worker_id).await?;
    executor
        .auto_update_worker(&fork.worker_id, component.revision, false)
        .await?;
    let outcomes =
        settled_update_outcomes(&executor, context.default_environment_id, &fork.worker_id).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    let oplog = executor
        .get_oplog(&fork.worker_id, OplogIndex::INITIAL)
        .await?;
    let first_outcome = outcomes[0].oplog_index;
    assert_only_update_entries(&entry_kinds(&oplog, forked));

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &fork.worker_id);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!executor.worker_is_loaded(&owned).await);
    executor
        .auto_update_worker(&fork.worker_id, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if update_outcomes(&executor, &fork.worker_id).await?.len() == 2
                && !executor.worker_is_loaded(&owned).await
            {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let oplog = executor
        .get_oplog(&fork.worker_id, OplogIndex::INITIAL)
        .await?;
    assert_only_update_entries(&entry_kinds(&oplog, first_outcome));
    assert_eq!(
        executor.get_worker_metadata(&fork.worker_id).await?.status,
        AgentStatus::Sandboxed
    );
    Ok(())
}

/// A full replay of a never-live fork lists its preopened root directory with a recorded
/// permission check, and the native listing is the only result it observes that the oplog does
/// not record. The listing taints the generation, so the divergence right after it proves no
/// incompatibility: the marked update fails as `Operational` and keeps the divergence. The
/// unmarked twin of the same cut is never tainted and fails as `Divergence` at the same index.
#[test]
#[timeout("4m")]
async fn a_divergence_after_a_replayed_directory_listing_of_a_never_live_fork_is_operational(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let source = agent_id!("DirectoryListing");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "list_then_draw", data_value!())
        .await?;
    let cut = tip(&executor, &source_id).await?;
    let target = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    let fork = || {
        ForkTarget::new(
            &component,
            phantom_agent_id!("DirectoryListing", uuid::Uuid::new_v4()),
        )
    };
    let (marked, twin) = (fork()?, fork()?);
    executor
        .fork_worker_never_live(&source_id, &marked.agent.to_string(), cut)
        .await?;
    executor
        .auto_update_worker(&marked.worker_id, target.revision, false)
        .await?;
    let outcomes =
        settled_update_outcomes(&executor, context.default_environment_id, &marked.worker_id)
            .await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    let PublicOplogEntry::FailedUpdate(marked_failure) = &outcomes[0].entry else {
        return Err(anyhow!("the marked update did not fail: {outcomes:?}"));
    };
    assert!(
        marked_failure.snapshot_assisted_details.is_none(),
        "{marked_failure:?}"
    );
    assert_eq!(
        marked_failure.failure_class,
        UpdateFailureClass::Operational,
        "{marked_failure:?}"
    );

    executor
        .fork_worker(&source_id, &twin.agent.to_string(), cut)
        .await?;
    executor
        .auto_update_worker(&twin.worker_id, target.revision, false)
        .await?;
    let twin_outcomes = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let outcomes = update_outcomes(&executor, &twin.worker_id).await?;
            if !outcomes.is_empty() {
                break anyhow::Ok(outcomes);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let PublicOplogEntry::FailedUpdate(twin_failure) = &twin_outcomes[0].entry else {
        return Err(anyhow!("the twin update did not fail: {twin_outcomes:?}"));
    };
    assert_eq!(
        twin_failure.failure_class,
        UpdateFailureClass::Divergence,
        "{twin_failure:?}"
    );
    let twin_divergence = twin_failure
        .divergence
        .as_ref()
        .map(|divergence| divergence.oplog_index);
    assert!(twin_divergence.is_some(), "{twin_failure:?}");
    assert_eq!(
        marked_failure
            .divergence
            .as_ref()
            .map(|divergence| divergence.oplog_index),
        twin_divergence
    );
    Ok(())
}

/// A quota service whose every reservation is throttled, with the estimated wait it holds:
/// a refill far in the future, or `None` as for a capacity resource.
struct ThrottlingQuotaService(Option<u64>);

#[async_trait::async_trait]
impl golem_worker_executor::services::quota::QuotaService for ThrottlingQuotaService {
    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: golem_common::model::quota::ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> golem_worker_executor::services::quota::LeaseInterest {
        golem_worker_executor::services::quota::QuotaService::acquire(
            &golem_worker_executor::services::quota::UnlimitedQuotaService,
            environment_id,
            resource_name,
            expected_use,
            previous_credit,
            previous_credit_at,
        )
        .await
    }

    async fn try_reserve(
        &self,
        _interest: &mut golem_worker_executor::services::quota::LeaseInterest,
        _amount: u64,
    ) -> golem_common::model::quota::ReserveResult {
        golem_common::model::quota::ReserveResult::InsufficientAllocation {
            enforcement_action: golem_common::model::quota::EnforcementAction::Throttle,
            estimated_wait_nanos: self.0,
        }
    }

    async fn commit(
        &self,
        interest: &mut golem_worker_executor::services::quota::LeaseInterest,
        reservation: golem_common::model::quota::Reservation,
        used: u64,
    ) {
        golem_worker_executor::services::quota::QuotaService::commit(
            &golem_worker_executor::services::quota::UnlimitedQuotaService,
            interest,
            reservation,
            used,
        )
        .await
    }
}

/// Scheduler storage that records the agent of every `Resume` it stores.
#[derive(Debug)]
struct RecordingResumes {
    inner: Arc<dyn golem_worker_executor::storage::scheduler::SchedulerStorage + Send + Sync>,
    resumed: Arc<std::sync::Mutex<Vec<AgentId>>>,
}

#[async_trait::async_trait]
impl golem_worker_executor::storage::scheduler::SchedulerStorage for RecordingResumes {
    async fn insert(
        &self,
        id: golem_common::model::ScheduleId,
        due: chrono::DateTime<chrono::Utc>,
        shard: golem_common::model::ShardId,
        action: &[u8],
    ) -> Result<(), golem_worker_executor::storage::scheduler::SchedulerStorageError> {
        if let golem_common::model::ScheduledAction::Resume { owned_agent_id, .. } =
            golem_common::serialization::deserialize(action).unwrap()
        {
            self.resumed.lock().unwrap().push(owned_agent_id.agent_id);
        }
        self.inner.insert(id, due, shard, action).await
    }

    async fn cancel(
        &self,
        id: &golem_common::model::ScheduleId,
    ) -> Result<(), golem_worker_executor::storage::scheduler::SchedulerStorageError> {
        self.inner.cancel(id).await
    }

    async fn claim_due(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        assignment: &golem_common::model::ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<
        Vec<golem_worker_executor::storage::scheduler::ClaimedScheduledAction>,
        golem_worker_executor::storage::scheduler::SchedulerStorageError,
    > {
        self.inner.claim_due(now, assignment, limit, ttl).await
    }

    async fn count_due(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        assignment: &golem_common::model::ShardAssignment,
    ) -> Result<u64, golem_worker_executor::storage::scheduler::SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        id: &golem_common::model::ScheduleId,
        owner: uuid::Uuid,
        until: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, golem_worker_executor::storage::scheduler::SchedulerStorageError> {
        self.inner.extend_lease(id, owner, until).await
    }

    async fn ack(
        &self,
        id: &golem_common::model::ScheduleId,
        owner: uuid::Uuid,
    ) -> Result<bool, golem_worker_executor::storage::scheduler::SchedulerStorageError> {
        self.inner.ack(id, owner).await
    }
}

/// A source agent is throttled by a quota reservation with a wait estimate: the recorded result
/// makes it schedule a `Resume` and suspend. A never-live fork of it replays that recorded
/// throttle on each automatic update, also after a cold restart of the executor. The fork refuses
/// the throttle, so the scheduler stores no `Resume` for it, and each update ends with one
/// `NotReplayable` failure and appends only its update entries.
#[test]
#[timeout("4m")]
async fn a_replayed_quota_throttle_of_a_never_live_fork_schedules_no_resume(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    replay_a_quota_throttle_on_a_never_live_fork(
        last_unique_id,
        deps,
        host_api_tests,
        Some(3_600_000_000_000),
    )
    .await
}

/// A throttle without a wait estimate, as for a capacity resource, suspends the source without a
/// `Resume`. A never-live fork refuses it too, so the replayed throttle does not suspend the fork
/// in the middle of its update: each update ends with one `NotReplayable` failure, also after a
/// cold restart of the executor.
#[test]
#[timeout("4m")]
async fn a_replayed_quota_throttle_without_a_wait_does_not_suspend_a_never_live_fork(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    replay_a_quota_throttle_on_a_never_live_fork(last_unique_id, deps, host_api_tests, None).await
}

async fn replay_a_quota_throttle_on_a_never_live_fork(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    estimated_wait_nanos: Option<u64>,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let resumed = Arc::new(std::sync::Mutex::new(Vec::<AgentId>::new()));
    let overrides = TestExecutorOverrides {
        quota_service: Some(Arc::new(ThrottlingQuotaService(estimated_wait_nanos))),
        wrap_scheduler_storage: Some(Arc::new({
            let resumed = resumed.clone();
            move |inner| {
                Arc::new(RecordingResumes {
                    inner,
                    resumed: resumed.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let source = agent_id!("QuotaApi", "never-live-throttle");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_agent(
            &component,
            &source,
            "reserve_and_commit",
            data_value!("never-live-rate".to_string(), 1u64, 1u64, 1u64),
        )
        .await?;
    executor
        .wait_for_status(&source_id, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    assert_eq!(
        resumed
            .lock()
            .unwrap()
            .iter()
            .filter(|agent| **agent == source_id)
            .count(),
        usize::from(estimated_wait_nanos.is_some()),
        "the source scheduled its resume only with a wait estimate"
    );
    let cut = tip(&executor, &source_id).await?;

    let fork = ForkTarget::new(
        &component,
        phantom_agent_id!("QuotaApi", uuid::Uuid::new_v4(), "never-live-throttle"),
    )?;
    executor
        .fork_worker_never_live(&source_id, &fork.agent.to_string(), cut)
        .await?;
    let forked = tip(&executor, &fork.worker_id).await?;
    executor
        .auto_update_worker(&fork.worker_id, component.revision, false)
        .await?;
    let outcomes =
        settled_update_outcomes(&executor, context.default_environment_id, &fork.worker_id).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    let first_outcome = outcomes[0].oplog_index;
    assert_only_update_entries(&entry_kinds(
        &executor
            .get_oplog(&fork.worker_id, OplogIndex::INITIAL)
            .await?,
        forked,
    ));

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &fork.worker_id);
    executor
        .auto_update_worker(&fork.worker_id, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if update_outcomes(&executor, &fork.worker_id).await?.len() == 2
                && !executor.worker_is_loaded(&owned).await
            {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let oplog = executor
        .get_oplog(&fork.worker_id, OplogIndex::INITIAL)
        .await?;
    assert_only_update_entries(&entry_kinds(&oplog, first_outcome));

    for outcome in update_outcomes(&executor, &fork.worker_id).await? {
        let PublicOplogEntry::FailedUpdate(failed) = &outcome.entry else {
            return Err(anyhow!("the update did not fail: {outcome:?}"));
        };
        assert_eq!(
            failed.failure_class,
            UpdateFailureClass::NotReplayable,
            "{failed:?}"
        );
        assert!(
            failed
                .details
                .as_deref()
                .is_some_and(|details| details.contains("throttle-resume")),
            "the refused boundary is not named: {failed:?}"
        );
    }
    assert!(
        !resumed.lock().unwrap().contains(&fork.worker_id),
        "the never-live fork scheduled a resume: {:?}",
        resumed.lock().unwrap()
    );
    assert_eq!(
        executor.get_worker_metadata(&fork.worker_id).await?.status,
        AgentStatus::Sandboxed
    );
    Ok(())
}
