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

//! The snapshot hooks of the `HookEffect` agent (host-api-tests): each agent's constructor names
//! the effect that its load hook makes against a counting test peer.

use super::peers::{TcpPeer, UdpPeer};
use super::{settled_update_outcomes_after, tip, update_outcomes};
use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::public_oplog_entry::FailedUpdateParams;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex, UpdateFailureClass};
use golem_common::model::{
    AgentId, NamedRetryPolicy, OplogIndex, OwnedAgentId, Predicate, PredicateValue, RdbmsPoolKey,
    RetryPolicy, TransactionId,
};
use golem_common::{agent_key, data_value};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor::services::rdbms::ignite::IgniteType;
use golem_worker_executor::services::rdbms::mysql::MysqlType;
use golem_worker_executor::services::rdbms::postgres::PostgresType;
use golem_worker_executor::services::rdbms::{
    DbResult, DbResultStream, DbTransaction, Rdbms, RdbmsError, RdbmsService, RdbmsStatus,
    RdbmsTransactionStatus, RdbmsType,
};
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const HOOK_EFFECT: &str = "HookEffect";

/// An executor that takes a snapshot after every invocation, lets agents use sockets, counts the
/// key-value storage operations by label, and keeps fork activation local.
struct Hooks {
    executor: TestWorkerExecutor,
    context: TestContext,
    component: ComponentDto,
    storage: KeyValueStorageFaults,
}

impl Hooks {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        host_api_tests: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        Self::start_with(last_unique_id, deps, host_api_tests, Default::default()).await
    }

    /// [`Hooks::start`] with the other overrides of `base`.
    async fn start_with(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        host_api_tests: &PrecompiledComponent,
        base: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let storage = KeyValueStorageFaults::default();
        let executor = Self::executor(deps, &context, &storage, base).await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        Ok(Self {
            executor,
            context,
            component,
            storage,
        })
    }

    /// The executor of [`Self::start_with`] over `context` and `storage`.
    async fn executor(
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        storage: &KeyValueStorageFaults,
        base: TestExecutorOverrides,
    ) -> anyhow::Result<TestWorkerExecutor> {
        crate::fork::start_with_local_resume_and(
            deps,
            context,
            TestExecutorOverrides {
                configure: Some(Arc::new(|config| {
                    config.oplog.default_snapshotting =
                        SnapshotPolicy::EveryNInvocation { count: 1 };
                })),
                wrap_key_value_storage: Some(Arc::new({
                    let storage = storage.clone();
                    move |inner| {
                        Arc::new(FaultInjectingKeyValueStorage::new(inner, storage.clone()))
                    }
                })),
                ..base
            }
            .with_agent_sockets(),
        )
        .await
    }

    /// Stops the executor and starts it again over the same storage, with `base` as in
    /// [`Self::start_with`].
    async fn restarted_with(
        self,
        deps: &WorkerExecutorTestDependencies,
        base: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let Self {
            executor,
            context,
            component,
            storage,
        } = self;
        drop(executor);
        let executor = Self::executor(deps, &context, &storage, base).await?;
        Ok(Self {
            executor,
            context,
            component,
            storage,
        })
    }

    /// Starts `HookEffect(effect, port)`, sets its value and waits for the snapshot after it.
    async fn source(&self, effect: &str, port: u16) -> anyhow::Result<OwnedAgentId> {
        let agent = agent_key!(HOOK_EFFECT, effect.to_string(), port);
        let source_id = self
            .executor
            .start_agent(&self.component.id, agent.clone())
            .await?;
        self.executor
            .invoke_and_await_agent(&self.component, &agent, "set", data_value!(7u64))
            .await?;
        self.wait_for_snapshot_after_last_invocation(&source_id)
            .await?;
        Ok(source_id)
    }

    /// Waits for the snapshot after the last `AgentInvocationFinished` of `source_id`.
    async fn wait_for_snapshot_after_last_invocation(
        &self,
        source_id: &OwnedAgentId,
    ) -> anyhow::Result<OplogIndex> {
        let finished = self
            .executor
            .get_oplog(source_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .rev()
            .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .map(|entry| entry.oplog_index)
            .ok_or_else(|| anyhow!("no invocation of {source_id} finished"))?;
        crate::hot_update::wait_for_snapshot_after(&self.executor, source_id, finished).await
    }

    /// Invokes `method` on the source and returns the index of its `AgentInvocationFinished`:
    /// a cut before the snapshot that follows it.
    async fn invoke_and_cut(
        &self,
        effect: &str,
        port: u16,
        source_id: &OwnedAgentId,
        method: &str,
    ) -> anyhow::Result<OplogIndex> {
        let agent = agent_key!(HOOK_EFFECT, effect.to_string(), port);
        let _ = self
            .executor
            .invoke_and_await_agent(&self.component, &agent, method, data_value!())
            .await;
        self.executor
            .get_oplog(source_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .rev()
            .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .map(|entry| entry.oplog_index)
            .ok_or_else(|| anyhow!("{method} did not finish"))
    }

    /// A newer revision of the component with the same code.
    async fn newer_revision(&self) -> anyhow::Result<ComponentDto> {
        self.executor
            .update_component(&self.component.id, "golem_it_host_api_tests_release")
            .await
    }

    /// Forks `source_id` at `cut` with the never-live mark and applies an automatic update to
    /// `revision`. Returns the update outcomes of the fork.
    async fn marked_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
        revision: golem_common::model::component::ComponentRevision,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        Ok(self.marked_fork_update(source_id, cut, revision).await?.1)
    }

    /// [`Self::marked_update`], which also returns the fork.
    async fn marked_fork_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
        revision: golem_common::model::component::ComponentRevision,
    ) -> anyhow::Result<(OwnedAgentId, Vec<PublicOplogEntryWithIndex>)> {
        let fork = self.executor.fork_worker_never_live(source_id, cut).await?;
        self.executor
            .auto_update_worker(&fork, revision, false)
            .await?;
        let outcomes = settled_update_outcomes_after(&self.executor, &fork, cut).await?;
        Ok((fork, outcomes))
    }

    /// Forks `source_id` at `cut` without the mark, applies an automatic update to `revision` and
    /// returns its first update outcome.
    async fn unmarked_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
        revision: golem_common::model::component::ComponentRevision,
    ) -> anyhow::Result<PublicOplogEntryWithIndex> {
        let twin = self.executor.fork_worker(source_id, cut).await?;
        self.executor
            .auto_update_worker(&twin, revision, false)
            .await?;
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                if let Some(outcome) = update_outcomes(&self.executor, &twin)
                    .await?
                    .into_iter()
                    .find(|outcome| outcome.oplog_index > cut)
                {
                    break anyhow::Ok(outcome);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }

    /// How many invocations the RPC peer `HookEffect("rpc-peer", port)` received; zero when the
    /// peer was never created.
    async fn rpc_peer_invocations(&self, port: u16) -> anyhow::Result<usize> {
        let peer = self.executor.keyed_agent_id(
            self.component.environment_id,
            &agent_key!(HOOK_EFFECT, "rpc-peer".to_string(), port),
        )?;
        if self
            .executor
            .get_worker_metadata_opt(&peer)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        Ok(self
            .executor
            .get_oplog(&peer, OplogIndex::INITIAL)
            .await?
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .count())
    }
}

/// The one outcome of `outcomes`, which must be a failed update.
fn the_failure(outcomes: &[PublicOplogEntryWithIndex]) -> anyhow::Result<&FailedUpdateParams> {
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    match &outcomes[0].entry {
        PublicOplogEntry::FailedUpdate(failed) => Ok(failed),
        _ => Err(anyhow!("the update did not fail: {outcomes:?}")),
    }
}

/// The first host call that a never-live fork refuses for the hook `effect`. An RPC client first
/// connects to its peer with `upsert`, which the never-live rule refuses before the read-only rule.
fn refused_boundary(effect: &str) -> &str {
    match effect {
        "rpc" => "golem::rpc::wasm-rpc::upsert",
        "http" => "http::client::send",
        other => other,
    }
}

fn assert_not_replayable(failed: &FailedUpdateParams, refused: &str) {
    assert_eq!(
        failed.failure_class,
        UpdateFailureClass::NotReplayable,
        "{failed:?}"
    );
    assert!(
        failed
            .details
            .as_deref()
            .is_some_and(|details| details.contains(refused)),
        "the refused boundary {refused} is not named: {failed:?}"
    );
}

/// E9(a), E9(c): a never-live fork, cut after a periodic snapshot record of its source, is
/// updated to a newer revision with the same code, so the record is the baseline and the load
/// hook runs. An HTTP request in the hook never reaches the peer, and the update is not
/// replayable. A hook without an effect replays.
#[test]
#[timeout("4m")]
async fn an_http_load_hook_of_a_never_live_fork_reaches_no_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let peer = TcpPeer::answering().await?;
    let source_id = hooks.source("http", peer.port).await?;
    let cut = tip(&hooks.executor, &source_id).await?;
    let target = hooks.newer_revision().await?;

    let (fork, outcomes) = hooks
        .marked_fork_update(&source_id, cut, target.revision)
        .await?;
    let failed = the_failure(&outcomes)?;
    assert!(failed.snapshot_assisted_details.is_some(), "{failed:?}");
    assert_not_replayable(failed, "http");
    // E3d, the hook-refusal leg.
    super::assert_stopped_for_good(&hooks.executor, &fork, 1).await?;
    assert_eq!(peer.connections(), 0);

    let harmless = hooks.source("none", peer.port).await?;
    let cut = tip(&hooks.executor, &harmless).await?;
    let target = hooks.newer_revision().await?;
    let outcomes = hooks.marked_update(&harmless, cut, target.revision).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    let PublicOplogEntry::SuccessfulUpdate(succeeded) = &outcomes[0].entry else {
        return Err(anyhow!("the harmless hook did not replay: {outcomes:?}"));
    };
    assert!(
        succeeded.snapshot_assisted_details.is_some(),
        "{succeeded:?}"
    );
    assert_eq!(peer.connections(), 0);
    Ok(())
}

/// E9(a): the same with an RPC call in the load hook. The peer agent is never created.
#[test]
#[timeout("4m")]
async fn an_rpc_load_hook_of_a_never_live_fork_reaches_no_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let port = 40_001;
    let source_id = hooks.source("rpc", port).await?;
    let cut = tip(&hooks.executor, &source_id).await?;
    let target = hooks.newer_revision().await?;

    let outcomes = hooks
        .marked_update(&source_id, cut, target.revision)
        .await?;
    let failed = the_failure(&outcomes)?;
    assert!(failed.snapshot_assisted_details.is_some(), "{failed:?}");
    assert_not_replayable(failed, refused_boundary("rpc"));
    assert_eq!(hooks.rpc_peer_invocations(port).await?, 0);
    Ok(())
}

/// E9(b): the source applies a manual update to a newer revision, so its `SuccessfulUpdate`
/// promotes the snapshot to the baseline; the source is no phantom, so its own load makes no
/// effect. A never-live fork of it gets a plain automatic update to a later revision: its load hook runs from the promoted baseline, and the HTTP
/// request, and separately the RPC call, in the hook reach no peer. The baseline is not skipped:
/// the update is not replayable.
#[test]
#[timeout("4m")]
async fn a_load_hook_from_a_promoted_snapshot_of_a_never_live_fork_reaches_no_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let peer = TcpPeer::answering().await?;
    let rpc_port = 40_002;
    let mut sources = Vec::new();
    for (effect, port) in [("http", peer.port), ("rpc", rpc_port)] {
        sources.push((effect, hooks.source(effect, port).await?));
    }
    let promoted = hooks.newer_revision().await?;
    let target = hooks.newer_revision().await?;
    for (effect, source_id) in sources {
        hooks
            .executor
            .manual_update_worker(&source_id, promoted.revision, false)
            .await?;
        hooks
            .executor
            .wait_for_component_revision(&source_id, promoted.revision, Duration::from_secs(60))
            .await?;
        let cut = tip(&hooks.executor, &source_id).await?;
        let outcomes = hooks
            .marked_update(&source_id, cut, target.revision)
            .await?;
        let failed = the_failure(&outcomes)?;
        assert_not_replayable(failed, refused_boundary(effect));
    }
    assert_eq!(peer.connections(), 0);
    assert_eq!(hooks.rpc_peer_invocations(rpc_port).await?, 0);
    Ok(())
}

/// E9(d), as Simon decided: an unmarked fork's load hook runs with the read-only rule, so its HTTP
/// request and its RPC call fail as read-only violations and reach no peer.
#[test]
#[timeout("4m")]
async fn the_load_hooks_of_unmarked_forks_fail_by_the_read_only_rule_and_reach_no_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let peer = TcpPeer::answering().await?;
    let rpc_port = 40_003;
    let mut sources = Vec::new();
    for (effect, port) in [("http", peer.port), ("rpc", rpc_port)] {
        sources.push(hooks.source(effect, port).await?);
    }
    let target = hooks.newer_revision().await?;
    for source_id in sources {
        let cut = tip(&hooks.executor, &source_id).await?;
        let outcome = hooks
            .unmarked_update(&source_id, cut, target.revision)
            .await?;
        let rendered = format!("{outcome:?}");
        assert!(
            rendered.to_lowercase().contains("read-only")
                || rendered.to_lowercase().contains("readonly"),
            "the load hook did not fail by the read-only rule: {rendered}"
        );
    }
    assert_eq!(peer.connections(), 0);
    assert_eq!(hooks.rpc_peer_invocations(rpc_port).await?, 0);
    Ok(())
}

/// E23: a load hook of a never-live fork that creates a promise, and one that completes an
/// existing promise of the source: no promise is created or completed, the source's promise
/// keeps no result, and each update is not replayable. The unmarked control, as for E9(d): an
/// unmarked fork's load hook fails by the read-only rule and writes no promise.
#[test]
#[timeout("4m")]
async fn promise_writes_in_a_load_hook_of_a_never_live_fork_are_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let port = 40_005;

    let mut sources = Vec::new();
    for effect in ["create-promise", "complete-promise"] {
        let agent = agent_key!(HOOK_EFFECT, effect.to_string(), port);
        let source_id = hooks
            .executor
            .start_agent(&hooks.component.id, agent.clone())
            .await?;
        hooks
            .executor
            .invoke_and_await_agent(&hooks.component, &agent, "prepare_promise", data_value!())
            .await?;
        hooks
            .wait_for_snapshot_after_last_invocation(&source_id)
            .await?;
        sources.push((effect, source_id));
    }
    let target = hooks.newer_revision().await?;
    for (effect, source_id) in sources {
        let cut = tip(&hooks.executor, &source_id).await?;
        let (created, completed) = (
            hooks.storage.calls("create"),
            hooks.storage.calls("complete"),
        );
        let outcomes = hooks
            .marked_update(&source_id, cut, target.revision)
            .await?;
        assert_not_replayable(the_failure(&outcomes)?, "promise");
        assert_eq!(hooks.storage.calls("create"), created, "{effect}");
        assert_eq!(hooks.storage.calls("complete"), completed, "{effect}");

        let outcome = hooks
            .unmarked_update(&source_id, cut, target.revision)
            .await?;
        assert!(
            format!("{outcome:?}").to_lowercase().contains("read-only"),
            "{outcome:?}"
        );
        assert_eq!(hooks.storage.calls("create"), created, "{effect}");
        assert_eq!(hooks.storage.calls("complete"), completed, "{effect}");
    }

    Ok(())
}

/// E24: a load hook of a never-live fork that derives a persistent card from its wallet is
/// refused before the card service, and the update is not replayable.
#[test]
#[timeout("4m")]
async fn a_card_derivation_in_a_load_hook_of_a_never_live_fork_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let port = 40_006;
    let source_id = hooks.source("card", port).await?;
    let cut = tip(&hooks.executor, &source_id).await?;
    let target = hooks.newer_revision().await?;
    let outcomes = hooks
        .marked_update(&source_id, cut, target.revision)
        .await?;
    assert_not_replayable(the_failure(&outcomes)?, "permissions");
    Ok(())
}

/// E14 and E21, the load-hook cases: a P3 TCP connect, a P3 UDP send and a P3 UDP receive in the
/// load hook of a never-live fork reach no peer, and each update is not replayable.
#[test]
#[timeout("4m")]
async fn sockets_in_a_load_hook_of_a_never_live_fork_reach_no_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let tcp = TcpPeer::answering().await?;
    let udp = UdpPeer::start().await?;
    let mut sources = Vec::new();
    for (effect, port) in [("tcp", tcp.port), ("udp", udp.port), ("udp-receive", 0)] {
        sources.push((effect, hooks.source(effect, port).await?));
    }
    let target = hooks.newer_revision().await?;
    let mut classes = Vec::new();
    for (effect, source_id) in sources {
        let cut = tip(&hooks.executor, &source_id).await?;
        let outcomes = hooks
            .marked_update(&source_id, cut, target.revision)
            .await?;
        let failed = the_failure(&outcomes)?;
        classes.push((effect, failed.failure_class, failed.details.clone()));
    }
    assert!(
        classes
            .iter()
            .all(|(_, class, _)| *class == UpdateFailureClass::NotReplayable),
        "{classes:#?}"
    );
    assert_eq!(tcp.connections(), 0);
    assert_eq!(udp.datagrams(), 0);
    Ok(())
}

/// E1: an HTTP request left incomplete by a real executor restart. A never-live fork cut after
/// its `Start`, updated to its own revision, sends nothing, appends no `End`, and writes one
/// `SuccessfulUpdate`. The control, an unmarked fork of the same cut, sends the request again.
#[test]
#[timeout("4m")]
async fn an_incomplete_http_request_of_a_never_live_fork_is_never_sent_again(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let peer = TcpPeer::silent().await?;
    let agent = agent_key!(HOOK_EFFECT, "none".to_string(), peer.port);
    let source_id = hooks
        .executor
        .start_agent(&hooks.component.id, agent.clone())
        .await?;
    hooks
        .executor
        .invoke_agent(&hooks.component, &agent, "call_http", data_value!())
        .await?;
    peer.wait_for(1).await?;
    let cut = incomplete_start(&hooks.executor, &source_id).await?;
    let hooks = hooks
        .restarted_with(deps, TestExecutorOverrides::default())
        .await?;
    // The source repairs its own incomplete request after the restart.
    peer.wait_for(2).await?;

    let outcomes = hooks
        .marked_update(&source_id, cut, hooks.component.revision)
        .await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    assert_eq!(peer.connections(), 2);

    hooks.executor.fork_worker(&source_id, cut).await?;
    peer.wait_for(3).await?;
    Ok(())
}

/// E1b: the same for an incomplete RPC call: the never-live fork writes one `SuccessfulUpdate` and
/// calls no peer. A fork that holds only its `Create` entry and the not yet started initialization
/// has no bootstrap block, so the first durable call of the initializer goes past the end of the
/// history: the update writes one `NotReplayable` failure, calls no peer and leaves no pending
/// update.
#[test]
#[timeout("4m")]
async fn an_incomplete_rpc_call_and_a_create_only_never_live_fork_replay_and_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    let port = 40_009;
    let agent = agent_key!(HOOK_EFFECT, "none".to_string(), port);
    let source_id = hooks
        .executor
        .start_agent(&hooks.component.id, agent.clone())
        .await?;
    hooks
        .executor
        .invoke_agent(&hooks.component, &agent, "call_rpc_wait", data_value!())
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while hooks.rpc_peer_invocations(port).await? == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let cut = incomplete_start(&hooks.executor, &source_id).await?;
    let hooks = hooks
        .restarted_with(deps, TestExecutorOverrides::default())
        .await?;
    // The source repairs its own incomplete call after the restart, which reaches the peer
    // again; the forks start only after that.
    tokio::time::timeout(Duration::from_secs(30), async {
        while hooks.rpc_peer_invocations(port).await? < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let peer_invocations = hooks.rpc_peer_invocations(port).await?;

    let outcomes = hooks
        .marked_update(&source_id, cut, hooks.component.revision)
        .await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );

    // The smallest cut the fork API accepts is 2: the `Create` entry and the pending invocation
    // of the agent's initialization, which never started.
    let create_only = OplogIndex::from_u64(2);
    let fork = hooks
        .executor
        .fork_worker_never_live(&source_id, create_only)
        .await?;
    hooks
        .executor
        .auto_update_worker(&fork, hooks.component.revision, false)
        .await?;
    let outcomes = settled_update_outcomes_after(&hooks.executor, &fork, create_only).await?;
    assert_eq!(
        the_failure(&outcomes)?.failure_class,
        UpdateFailureClass::NotReplayable
    );
    let metadata = hooks.executor.get_worker_metadata(&fork).await?;
    assert!(
        !metadata.updates.iter().any(|record| matches!(
            record,
            golem_common::model::agent::UpdateRecord::PendingUpdate(_)
        )),
        "{:?}",
        metadata.updates
    );
    assert_eq!(hooks.rpc_peer_invocations(port).await?, peer_invocations);
    Ok(())
}

/// The index of the last `Start` of `agent_id` that has no `End`.
async fn incomplete_start(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(agent_id, OplogIndex::INITIAL).await?;
            let ended: Vec<OplogIndex> = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) => Some(end.start_index),
                    _ => None,
                })
                .collect();
            if let Some(start) = oplog.iter().rev().find(|entry| {
                matches!(entry.entry, PublicOplogEntry::Start(_))
                    && !ended.contains(&entry.oplog_index)
            }) {
                break anyhow::Ok(start.oplog_index);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

/// Forks the `HookEffect(effect, port)` source cut after `fail_if_restored` with the mark, updates
/// it to a newer revision with the same code, so that its load hook runs, and returns the class
/// of the one failed update. The source runs on the latest revision, and the target revision is
/// uploaded after the cut, so the update selects the source's snapshot.
async fn load_hook_failure_class(
    hooks: &Hooks,
    effect: &str,
    port: u16,
) -> anyhow::Result<UpdateFailureClass> {
    let source_id = hooks.source(effect, port).await?;
    let cut = hooks
        .invoke_and_cut(effect, port, &source_id, "fail_if_restored")
        .await?;
    let target = hooks.newer_revision().await?;
    let outcomes = hooks
        .marked_update(&source_id, cut, target.revision)
        .await?;
    let failed = the_failure(&outcomes)?;
    assert!(
        failed.snapshot_assisted_details.is_some(),
        "{effect}: {failed:?}"
    );
    Ok(failed.failure_class)
}

/// E25(e), the async-task half: the load hook reads fresh random bytes in a task it spawns, which
/// the fork may do as the hook itself may, and the replayed suffix then fails in the target code:
/// the update is a target trap.
#[test]
#[timeout("4m")]
async fn a_random_read_in_a_task_of_a_load_hook_is_admitted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let hooks = Hooks::start(last_unique_id, deps, host_api_tests).await?;
    assert_eq!(
        load_hook_failure_class(&hooks, "random-in-task", 40_010).await?,
        UpdateFailureClass::TargetTrap
    );
    Ok(())
}

#[test_r::test_dep(scope = Shared)]
async fn never_live_postgres() -> DockerPostgresRdb {
    DockerPostgresRdb::new(&uuid::Uuid::new_v4().to_string(), false).await
}

/// Every call that reaches the PostgreSQL service, by operation and agent. A call here is an
/// attempt to reach the database.
#[derive(Clone, Default)]
struct DatabaseCalls {
    calls: Arc<std::sync::Mutex<Vec<(&'static str, AgentId)>>>,
    failing_status: Arc<std::sync::Mutex<Vec<AgentId>>>,
}

impl DatabaseCalls {
    fn record(&self, operation: &'static str, agent_id: &AgentId) {
        self.calls.lock().unwrap().push((operation, *agent_id));
    }

    fn count(&self, operation: Option<&str>, agent_id: &AgentId) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(op, agent)| agent == agent_id && operation.is_none_or(|wanted| *op == wanted))
            .count()
    }

    fn total(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// The status query of `agent_id` fails with an error that the executor does not retry.
    fn fail_status_of(&self, agent_id: &AgentId) {
        self.failing_status.lock().unwrap().push(*agent_id);
    }

    fn overrides(&self) -> TestExecutorOverrides {
        let calls = self.clone();
        TestExecutorOverrides {
            wrap_rdbms_service: Some(Arc::new(move |inner| {
                Arc::new(CountingRdbms {
                    postgres: Arc::new(CountingPostgres {
                        inner: inner.postgres(),
                        calls: calls.clone(),
                    }),
                    inner,
                })
            })),
            ..Default::default()
        }
    }
}

struct CountingRdbms {
    inner: Arc<dyn RdbmsService>,
    postgres: Arc<CountingPostgres>,
}

impl RdbmsService for CountingRdbms {
    fn ignite(&self) -> Arc<dyn Rdbms<IgniteType>> {
        self.inner.ignite()
    }
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>> {
        self.inner.mysql()
    }
    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>> {
        self.postgres.clone()
    }
}

struct CountingPostgres {
    inner: Arc<dyn Rdbms<PostgresType>>,
    calls: DatabaseCalls,
}

type PgValue = <PostgresType as RdbmsType>::DbValue;

#[async_trait::async_trait]
impl Rdbms<PostgresType> for CountingPostgres {
    async fn create(&self, address: &str, agent_id: &AgentId) -> Result<RdbmsPoolKey, RdbmsError> {
        self.calls.record("create", agent_id);
        self.inner.create(address, agent_id).await
    }
    async fn exists(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.inner.exists(key, agent_id).await
    }
    async fn remove(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.inner.remove(key, agent_id).await
    }
    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<PgValue>,
    ) -> Result<u64, RdbmsError> {
        self.calls.record("execute", agent_id);
        self.inner.execute(key, agent_id, statement, params).await
    }
    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<PgValue>,
    ) -> Result<Arc<dyn DbResultStream<PostgresType> + Send + Sync>, RdbmsError> {
        self.calls.record("query_stream", agent_id);
        self.inner
            .query_stream(key, agent_id, statement, params)
            .await
    }
    async fn query(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<PgValue>,
    ) -> Result<DbResult<PostgresType>, RdbmsError> {
        self.calls.record("query", agent_id);
        self.inner.query(key, agent_id, statement, params).await
    }
    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
    ) -> Result<Arc<dyn DbTransaction<PostgresType> + Send + Sync>, RdbmsError> {
        self.calls.record("begin", agent_id);
        self.inner.begin_transaction(key, agent_id).await
    }
    async fn get_transaction_status(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        self.calls.record("status", agent_id);
        if self.calls.failing_status.lock().unwrap().contains(agent_id) {
            return Err(RdbmsError::Other("the status query fails".to_string()));
        }
        self.inner
            .get_transaction_status(key, agent_id, transaction_id)
            .await
    }
    async fn cleanup_transaction(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<(), RdbmsError> {
        self.calls.record("cleanup", agent_id);
        self.inner
            .cleanup_transaction(key, agent_id, transaction_id)
            .await
    }
    async fn status(&self) -> RdbmsStatus {
        self.inner.status().await
    }
}

fn postgres_port(postgres: &DockerPostgresRdb) -> anyhow::Result<u16> {
    let address = postgres.public_connection_string();
    let (_, rest) = address
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("no port in {address}"))?;
    let (port, _) = rest
        .split_once('/')
        .ok_or_else(|| anyhow!("no database in {address}"))?;
    Ok(port.parse()?)
}

/// E17(a): a load hook that opens a PostgreSQL connection and begins a transaction. On a
/// never-live fork the transaction scope is refused: the database service gets no call, and the
/// update fails as `NotReplayable`. The control, an unmarked twin of the same cut, begins the transaction at
/// the database.
#[test]
#[timeout("5m")]
async fn a_never_live_load_hook_begins_no_database_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    never_live_postgres: &DockerPostgresRdb,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let calls = DatabaseCalls::default();
    let hooks = Hooks::start_with(last_unique_id, deps, host_api_tests, calls.overrides()).await?;
    let port = postgres_port(never_live_postgres)?;
    let source_id = hooks.source("rdbms-begin", port).await?;
    let cut = tip(&hooks.executor, &source_id).await?;
    let target = hooks.newer_revision().await?;

    let outcomes = hooks
        .marked_update(&source_id, cut, target.revision)
        .await?;
    let failed = the_failure(&outcomes)?;
    assert!(failed.snapshot_assisted_details.is_some(), "{failed:?}");
    assert_not_replayable(failed, "<scope:transaction>");
    assert_eq!(calls.total(), 0);

    let twin = hooks
        .unmarked_update(&source_id, cut, target.revision)
        .await?;
    assert!(
        calls
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| *op == "begin"),
        "the unmarked load hook did not begin its transaction: {twin:?}"
    );
    Ok(())
}

/// The index of the one entry of `agent_id` that `matches` selects.
async fn the_entry(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    matches: impl Fn(&PublicOplogEntry) -> bool,
) -> anyhow::Result<OplogIndex> {
    let oplog = executor.get_oplog(agent_id, OplogIndex::INITIAL).await?;
    let mut found = oplog.iter().filter(|entry| matches(&entry.entry));
    let entry = found
        .next()
        .ok_or_else(|| anyhow!("no matching entry in {oplog:?}"))?;
    assert!(found.next().is_none(), "{oplog:?}");
    Ok(entry.oplog_index)
}

/// Overrides whose RDBMS service counts every call in `calls`, and whose retry policy gives a
/// failed transaction status query up at once.
fn database_overrides(calls: &DatabaseCalls) -> TestExecutorOverrides {
    TestExecutorOverrides {
        retry_policies: Some(vec![
            NamedRetryPolicy {
                name: "status-give-up".to_string(),
                priority: 100,
                predicate: Predicate::PropEq {
                    property: "verb".to_string(),
                    value: PredicateValue::Text("transaction-status".to_string()),
                },
                policy: RetryPolicy::Never,
            },
            NamedRetryPolicy {
                name: "permissive-trap".to_string(),
                priority: 0,
                predicate: Predicate::True,
                policy: RetryPolicy::CountBox {
                    max_retries: 5,
                    inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
                },
            },
        ]),
        ..calls.overrides()
    }
}

/// [`Hooks`] with [`database_overrides`].
async fn database_hooks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    calls: &DatabaseCalls,
) -> anyhow::Result<Hooks> {
    Hooks::start_with(
        last_unique_id,
        deps,
        host_api_tests,
        database_overrides(calls),
    )
    .await
}

/// A source that committed a PostgreSQL transaction at the database and is held before it
/// records the commit: its oplog ends with the `PreCommitRemoteTransaction` marker. A fork
/// cannot cut between the marker and a recorded terminal, so the forks of the marker cut are made
/// while the source is held.
struct HeldCommit {
    source_id: OwnedAgentId,
    pre_commit: OplogIndex,
    held: golem_worker_executor_test_utils::AgentInitializationEnqueueGateHandle,
    commit: tokio::task::JoinHandle<anyhow::Result<golem_test_framework::dsl::AgentResult>>,
}

impl HeldCommit {
    /// Starts `agent_type`, invokes `touch` first when `touch` is set and waits for the snapshot
    /// after it, then commits a transaction and holds the source at the marker.
    async fn start(
        hooks: &Hooks,
        postgres: &DockerPostgresRdb,
        agent_type: &'static str,
        touch: bool,
    ) -> anyhow::Result<Self> {
        let address = postgres.public_connection_string();
        let pool = sqlx::PgPool::connect(&address).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS completion_effects (label TEXT)")
            .execute(&pool)
            .await?;
        let name = format!("never-live-rdbms-{}", uuid::Uuid::new_v4());
        let source = agent_key!(agent_type, name.clone());
        let source_id = hooks
            .executor
            .start_agent(&hooks.component.id, source.clone())
            .await?;
        if touch {
            hooks
                .executor
                .invoke_and_await_agent(&hooks.component, &source, "touch", data_value!())
                .await?;
            hooks
                .wait_for_snapshot_after_last_invocation(&source_id)
                .await?;
        }
        let mut held = hooks
            .executor
            .gate_next_remote_transaction_terminal_append(&source_id.agent_id)
            .await;
        let commit = {
            let executor = hooks.executor.clone();
            let component = hooks.component.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(&component, &source, "commit", data_value!(address))
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(60), held.entered()).await?;
        let pre_commit = the_entry(&hooks.executor, &source_id, |entry| {
            matches!(entry, PublicOplogEntry::PreCommitRemoteTransaction(_))
        })
        .await?;
        assert_eq!(tip(&hooks.executor, &source_id).await?, pre_commit);
        Ok(Self {
            source_id,
            pre_commit,
            held,
            commit,
        })
    }

    /// Lets the source record its commit and returns its tip after the commit.
    async fn finish(self, hooks: &Hooks) -> anyhow::Result<OplogIndex> {
        drop(self.held);
        self.commit.await??;
        tip(&hooks.executor, &self.source_id).await
    }
}

/// E17(b), E17(c) and N5: a source commits a PostgreSQL transaction.
///
/// - A never-live fork cut at the `PreCommitRemoteTransaction` marker, which has no terminal,
///   updated to its own revision: the database service gets no call, the fork appends no
///   transaction terminal, and the update succeeds.
/// - A never-live fork of the completed transaction replays it with no database call.
/// - Control: an unmarked fork of the same marker cut resolves the incomplete terminal with one
///   status query and records the commit. The unmarked fork whose status query fails
///   permanently is in the E20 test below.
#[test]
#[timeout("6m")]
async fn a_marked_fork_replays_a_postgres_commit_without_reaching_the_database(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    never_live_postgres: &DockerPostgresRdb,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let calls = DatabaseCalls::default();
    let hooks = database_hooks(last_unique_id, deps, host_api_tests, &calls).await?;
    let source = HeldCommit::start(&hooks, never_live_postgres, "DbKvCompletion", false).await?;
    let source_id = source.source_id.clone();
    let pre_commit = source.pre_commit;
    let marked_at_marker = hooks
        .executor
        .fork_worker_never_live(&source_id, pre_commit)
        .await?;
    let twin = hooks.executor.fork_worker(&source_id, pre_commit).await?;
    let completed = source.finish(&hooks).await?;
    let marked_completed = hooks
        .executor
        .fork_worker_never_live(&source_id, completed)
        .await?;

    for (fork, cut) in [
        (marked_at_marker, pre_commit),
        (marked_completed, completed),
    ] {
        hooks
            .executor
            .auto_update_worker(&fork, hooks.component.revision, false)
            .await?;
        let outcomes = settled_update_outcomes_after(&hooks.executor, &fork, cut).await?;
        assert_eq!(outcomes.len(), 1, "{cut}: {outcomes:?}");
        assert!(
            matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
            "{cut}: {outcomes:?}"
        );
        let oplog = hooks.executor.get_oplog(&fork, OplogIndex::INITIAL).await?;
        // The fork writes its own `StreamSession` record at a cut inside an invocation, and the
        // update its `PendingUpdate` entries. Nothing else: no transaction terminal and no call.
        let appended = super::entry_kinds(&oplog, cut);
        assert!(
            appended.iter().all(|kind| matches!(
                kind.as_str(),
                "StreamSession" | "PendingUpdate" | "SuccessfulUpdate"
            )),
            "{cut}: {appended:?}"
        );
        assert_eq!(calls.count(None, &fork.agent_id), 0, "{cut}");
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let oplog = hooks.executor.get_oplog(&twin, OplogIndex::INITIAL).await?;
            if oplog.iter().any(|entry| {
                entry.oplog_index > pre_commit
                    && matches!(entry.entry, PublicOplogEntry::CommittedRemoteTransaction(_))
            }) {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;
    assert_eq!(calls.count(Some("status"), &twin.agent_id), 1);
    Ok(())
}

/// E20, the classified permanent host failure, and the N5 control: an unmarked fork cut at a
/// `PreCommitRemoteTransaction` marker with no terminal, whose transaction status query fails
/// permanently, updated to a newer revision. The update writes one `FailedUpdate` of class
/// `Operational`, in a full replay (`DbKvCompletion`) and in a snapshot-assisted replay whose
/// transaction lies after the snapshot (`SnapshotDbCommit`). A never-live fork makes no status
/// query (E17(b)), so this case is unmarked.
#[test]
#[timeout("6m")]
async fn a_permanently_failing_transaction_status_query_fails_an_update_as_operational(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    never_live_postgres: &DockerPostgresRdb,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let calls = DatabaseCalls::default();
    let hooks = database_hooks(last_unique_id, deps, host_api_tests, &calls).await?;
    let mut failing = Vec::new();
    for (agent_type, assisted) in [("DbKvCompletion", false), ("SnapshotDbCommit", true)] {
        let source = HeldCommit::start(&hooks, never_live_postgres, agent_type, assisted).await?;
        // The status query of an unmarked fork runs as soon as the fork starts, so its ID is
        // chosen, and its status query made to fail, before the fork.
        let target = AgentId::random(
            source
                .source_id
                .agent_id
                .type_id()
                .ok_or_else(|| anyhow!("{} has no agent type", source.source_id))?,
        );
        calls.fail_status_of(&target);
        let fork = hooks
            .executor
            .fork_worker_to(&source.source_id, &target, source.pre_commit)
            .await?;
        let pre_commit = source.pre_commit;
        source.finish(&hooks).await?;
        failing.push((agent_type, assisted, fork, pre_commit));
    }
    // An unmarked fork starts at once. Its own start fails at the status query, and a start
    // without a pending update reads that failure after a snapshot as a snapshot divergence: it
    // rejects the snapshot records for the incarnation. A real restart ends the incarnation, and
    // the failed fork does not start again, so the update's start is the first one that may
    // select a record.
    for (agent_type, _, fork, pre_commit) in &failing {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let oplog = hooks.executor.get_oplog(fork, OplogIndex::INITIAL).await?;
                if oplog.iter().any(|entry| {
                    entry.oplog_index > *pre_commit
                        && matches!(entry.entry, PublicOplogEntry::Error(_))
                }) {
                    break anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?
        .map_err(|error| anyhow!("{agent_type}: {error}"))?;
    }
    let hooks = hooks
        .restarted_with(deps, database_overrides(&calls))
        .await?;
    let newer = hooks.newer_revision().await?;
    for (agent_type, assisted, fork, pre_commit) in failing {
        hooks
            .executor
            .auto_update_worker(&fork, newer.revision, false)
            .await?;
        let outcome = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let Some(outcome) = update_outcomes(&hooks.executor, &fork)
                    .await?
                    .into_iter()
                    .find(|outcome| outcome.oplog_index > pre_commit)
                {
                    break anyhow::Ok(outcome);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await??;
        let PublicOplogEntry::FailedUpdate(failed) = &outcome.entry else {
            return Err(anyhow!(
                "{agent_type}: the update did not fail: {outcome:?}"
            ));
        };
        assert_eq!(
            failed.failure_class,
            UpdateFailureClass::Operational,
            "{agent_type}: {failed:?}"
        );
        assert_eq!(
            failed.snapshot_assisted_details.is_some(),
            assisted,
            "{agent_type}: {failed:?}"
        );
        assert!(
            calls.count(Some("status"), &fork.agent_id) >= 1,
            "{agent_type}"
        );
    }
    Ok(())
}
