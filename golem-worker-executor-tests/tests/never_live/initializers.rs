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

//! The core initializer of a never-live fork's target revision (`agent-initializer`): each
//! variant of the component is a later revision whose initializer adds one step after the
//! recorded environment read.

use super::{assert_stopped_for_good, settled_update_outcomes_after, tip};
use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::oplog::public_oplog_entry::FailedUpdateParams;
use golem_common::model::oplog::{
    PublicOplogEntry, PublicOplogEntryWithIndex, ReplayStep, UpdateFailureClass,
};
use golem_common::model::{OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_initializer")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

struct Initializers {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    /// The reads of the bucket `snapshot-load` that reach the key-value service.
    reads: Arc<AtomicUsize>,
}

impl Initializers {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        agent_initializer: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let executor = crate::fork::start_with_local_resume_and(
            deps,
            &context,
            TestExecutorOverrides {
                wrap_key_value_service: Some(Arc::new(move |inner| {
                    Arc::new(super::CountingKeyValueService {
                        inner,
                        reads: counted.clone(),
                    })
                })),
                configure: Some(Arc::new(|config| {
                    config.oplog.default_snapshotting =
                        SnapshotPolicy::EveryNInvocation { count: 1 };
                })),
                ..Default::default()
            },
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_initializer)
            .store()
            .await?;
        Ok(Self {
            executor,
            component,
            reads,
        })
    }

    /// Starts `agent_type("source")` with `PEER_PORT=port`, increments it once and returns it.
    async fn source(
        &self,
        agent_type: &str,
        name: &str,
        port: u16,
    ) -> anyhow::Result<OwnedAgentId> {
        let agent = agent_key!(agent_type, name.to_string());
        let source_id = self
            .executor
            .start_agent_with(
                &self.component.id,
                agent.clone(),
                HashMap::from([("PEER_PORT".to_string(), port.to_string())]),
                Vec::new(),
            )
            .await?;
        self.executor
            .invoke_and_await_agent(&self.component, &agent, "increment", data_value!())
            .await?;
        Ok(source_id)
    }

    async fn revision(&self, variant: &str) -> anyhow::Result<ComponentRevision> {
        let wasm = if variant == "base" {
            "it_agent_initializer_release".to_string()
        } else {
            format!("it_agent_initializer_{variant}_release")
        };
        Ok(self
            .executor
            .update_component(&self.component.id, &wasm)
            .await?
            .revision)
    }

    /// Forks `source_id` at `cut` with the never-live mark, updates the fork to `revision` and
    /// returns its update outcomes once it unloaded.
    async fn marked_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
        revision: ComponentRevision,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        Ok(self.marked_fork_update(source_id, cut, revision).await?.1)
    }

    /// [`Self::marked_update`], which also returns the fork.
    async fn marked_fork_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
        revision: ComponentRevision,
    ) -> anyhow::Result<(OwnedAgentId, Vec<PublicOplogEntryWithIndex>)> {
        let fork = self.executor.fork_worker_never_live(source_id, cut).await?;
        self.executor
            .auto_update_worker(&fork, revision, false)
            .await?;
        let outcomes = settled_update_outcomes_after(&self.executor, &fork, cut).await?;
        Ok((fork, outcomes))
    }

    /// The index of the last snapshot record of `source_id`.
    async fn snapshot_cut(&self, source_id: &OwnedAgentId) -> anyhow::Result<OplogIndex> {
        crate::hot_update::wait_for_snapshot_after(&self.executor, source_id, OplogIndex::INITIAL)
            .await?;
        tip(&self.executor, source_id).await
    }
}

fn the_failure(outcomes: &[PublicOplogEntryWithIndex]) -> anyhow::Result<&FailedUpdateParams> {
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    match &outcomes[0].entry {
        PublicOplogEntry::FailedUpdate(failed) => Ok(failed),
        _ => Err(anyhow!("the update did not fail: {outcomes:?}")),
    }
}

/// E15: a never-live fork updated to a revision whose initializer adds a key-value read after
/// the recorded calls, in a full and in a snapshot-assisted update. The added call has no
/// recorded `Start` to claim, so the replay stops at it before the gate is reached: the store
/// counts no read, and each update writes exactly one `FailedUpdate` of class `Divergence` whose
/// index is the entry the source recorded after its initializer. No runtime text, and no
/// pending update is left. The added call is a read, not an
/// HTTP request: the core initializer is a synchronous task, which may not wait for a P3 HTTP
/// send. The refusal of an executing initializer call is in the E3d test below.
#[test]
#[timeout("4m")]
async fn a_key_value_read_added_to_the_initializer_of_a_never_live_fork_diverges(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_initializer")] agent_initializer: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let initializers = Initializers::start(last_unique_id, deps, agent_initializer).await?;
    let full = initializers.source("Initialized", "e15", 15).await?;
    let full_cut = tip(&initializers.executor, &full).await?;
    let assisted = initializers
        .source("SnapshotInitialized", "e15", 15)
        .await?;
    let assisted_cut = initializers.snapshot_cut(&assisted).await?;
    let revision = initializers.revision("key-value").await?;

    for (agent_type, source_id, cut) in [
        ("Initialized", &full, full_cut),
        ("SnapshotInitialized", &assisted, assisted_cut),
    ] {
        let source_oplog = initializers
            .executor
            .get_oplog(source_id, OplogIndex::INITIAL)
            .await?;
        let after_initializer = source_oplog
            .iter()
            .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .ok_or_else(|| anyhow!("{agent_type}: the source has no invocation"))?
            .oplog_index;
        let (fork, outcomes) = initializers
            .marked_fork_update(source_id, cut, revision)
            .await?;
        let failed = the_failure(&outcomes)?;
        assert_eq!(
            failed.failure_class,
            UpdateFailureClass::Divergence,
            "{agent_type}: {failed:?}"
        );
        let divergence = failed
            .divergence
            .as_ref()
            .ok_or_else(|| anyhow!("{agent_type}: no divergence: {failed:?}"))?;
        assert_eq!(
            divergence.oplog_index, after_initializer,
            "{agent_type}: {failed:?}"
        );
        // A full replay claims the read on the main cursor, which names the call. A
        // snapshot-assisted replay runs the initializer on its bootstrap cursor, which reports a
        // call after its recorded bootstrap block.
        match &divergence.actual {
            ReplayStep::HostCall(actual) if agent_type == "Initialized" => {
                assert_eq!(
                    actual.function_name, "keyvalue::eventual::get",
                    "{agent_type}"
                )
            }
            ReplayStep::Other(other) if agent_type == "SnapshotInitialized" => assert!(
                other
                    .description
                    .contains("after its recorded bootstrap block"),
                "{agent_type}: {failed:?}"
            ),
            _ => return Err(anyhow!("{agent_type}: the actual step: {failed:?}")),
        }
        assert!(
            !failed
                .details
                .as_deref()
                .unwrap_or_default()
                .contains("guest call settlement task failed"),
            "{agent_type}: runtime text: {failed:?}"
        );
        assert_stopped_for_good(&initializers.executor, &fork, 1).await?;
    }
    assert_eq!(initializers.reads.load(Ordering::SeqCst), 0);
    Ok(())
}

/// E22: the initializer of the target revision panics in a full update of a never-live fork.
/// The update writes exactly one `FailedUpdate`, a target trap: a failure that is not a
/// refusal keeps its class.
#[test]
#[timeout("4m")]
async fn a_panic_in_the_initializer_of_a_never_live_fork_is_a_target_trap(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_initializer")] agent_initializer: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let initializers = Initializers::start(last_unique_id, deps, agent_initializer).await?;
    let source = initializers.source("Initialized", "e22", 9).await?;
    let cut = tip(&initializers.executor, &source).await?;
    let revision = initializers.revision("panic").await?;
    let outcomes = initializers.marked_update(&source, cut, revision).await?;
    let failed = the_failure(&outcomes)?;
    assert_eq!(
        failed.failure_class,
        UpdateFailureClass::TargetTrap,
        "{failed:?}"
    );
    Ok(())
}

/// E3d: a never-live fork whose last update fails by an initializer refusal stops for good: one
/// `FailedUpdate`, no pending update, and no instance is created after the failure. The fork
/// is cut after its `Create` only, so the target's initializer executes past the end of the
/// recorded history: its first durable call meets the live gate of the never-live agent. The store counts no read, and the failure is
/// classed as the never-live refusal, not as a failed instantiation. Of two updates queued together, where the
/// first fails, the second runs once in a later generation and succeeds.
#[test]
#[timeout("4m")]
async fn a_failed_update_of_a_never_live_fork_starts_no_further_generation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_initializer")] agent_initializer: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let initializers = Initializers::start(last_unique_id, deps, agent_initializer).await?;
    let source = initializers.source("Initialized", "e3d", 9).await?;
    let cut = tip(&initializers.executor, &source).await?;
    let refused = initializers.revision("key-value").await?;
    let plain = initializers.revision("base").await?;

    // The smallest cut the fork API accepts: the `Create` entry and the pending invocation of
    // the agent's initialization, which never started.
    let create_only = OplogIndex::from_u64(2);
    let (fork, outcomes) = initializers
        .marked_fork_update(&source, create_only, refused)
        .await?;
    let failed = the_failure(&outcomes)?;
    assert_eq!(
        failed.failure_class,
        UpdateFailureClass::NotReplayable,
        "{failed:?}"
    );
    let details = failed.details.as_deref().unwrap_or_default();
    assert!(
        details.contains("UPDATE_NOT_REPLAYABLE") && details.contains("never-live"),
        "{failed:?}"
    );
    assert!(
        !details.to_lowercase().contains("instantiat"),
        "the refusal is classed as a failed instantiation: {failed:?}"
    );
    assert_eq!(initializers.reads.load(Ordering::SeqCst), 0);
    assert_stopped_for_good(&initializers.executor, &fork, 1).await?;

    let queued = initializers
        .executor
        .fork_worker_never_live(&source, cut)
        .await?;
    initializers
        .executor
        .auto_update_worker(&queued, refused, true)
        .await?;
    initializers
        .executor
        .auto_update_worker(&queued, plain, false)
        .await?;
    let outcomes =
        super::wait_for_update_outcomes(&initializers.executor, &queued, OplogIndex::NONE, 2, true)
            .await?;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::FailedUpdate(_)),
        "{outcomes:?}"
    );
    assert!(
        matches!(outcomes[1].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    // One generation for each update: the failed one and the later successful one.
    assert_stopped_for_good(&initializers.executor, &queued, 2).await?;
    Ok(())
}
