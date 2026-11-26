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

//! The durable stream maintenance of a never-live fork of a stream consumer (`InputGc` of
//! agent-rpc-rust): the fork holds the source's cancellation intent, which it never forwards to
//! the producer, and its deletion touches nothing of the source.

use super::{update_outcomes, wait_for_update_outcomes};
use crate::Tracing;
use golem_common::model::durable_stream::StreamSessionRecord;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentStatus, OplogIndex, PromiseId};
use golem_common::schema::FromSchema;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// An executor whose RPC counts every durable stream attachment control in `controls`.
async fn start(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    controls: &Arc<AtomicUsize>,
) -> anyhow::Result<TestWorkerExecutor> {
    let controls = controls.clone();
    crate::fork::start_with_local_resume_and(
        deps,
        context,
        TestExecutorOverrides {
            wrap_rpc: Some(Arc::new(move |inner| {
                crate::fork::counting_stream_controls(inner, controls.clone())
            })),
            ..Default::default()
        },
    )
    .await
}

fn session_records(
    oplog: &[PublicOplogEntryWithIndex],
    matches: fn(&StreamSessionRecord) -> bool,
) -> impl Iterator<Item = &PublicOplogEntryWithIndex> {
    oplog.iter().filter(move |entry| {
        matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
            if StreamSessionRecord::from_value(session.record.value())
                .is_ok_and(|record| matches(&record)))
    })
}

fn is_cancel_intent(record: &StreamSessionRecord) -> bool {
    matches!(record, StreamSessionRecord::ConsumerCancelIntent(_))
}

fn is_cancel_applied(record: &StreamSessionRecord) -> bool {
    matches!(record, StreamSessionRecord::ConsumerCancelApplied(_))
}

/// E10 and E16: a stream consumer drops its stream before its next host call, which records a
/// cancellation intent for the producer, and then waits on a promise. A never-live fork cut right
/// after the intent holds it unresolved. Across the fork, an update in a cold acquisition, a real
/// executor restart, a second update, and the deletion of the fork, the fork sends no stream
/// attachment control and appends no `ConsumerCancelApplied`, and the source's stream records are
/// unchanged. The control, an unmarked fork of the same cut, forwards the intent.
#[test]
#[timeout("6m")]
async fn a_never_live_fork_of_a_stream_consumer_forwards_no_cancellation_and_its_deletion_touches_no_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let controls = Arc::new(AtomicUsize::new(0));
    let executor = start(deps, &context, &controls).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "never-live-stream".to_string();
    executor
        .start_agent(
            &component.id,
            agent_key!("StreamingRpcTarget", name.clone()),
        )
        .await?;
    let source = agent_key!("InputGc", name.clone());
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "configure", data_value!(0u32, 0u32))
        .await?;
    let gate: PromiseId = executor
        .invoke_and_await_agent(&component, &source, "promise", data_value!())
        .await?
        .into_typed()?;
    executor
        .invoke_agent(
            &component,
            &source,
            "run",
            data_value!(1u32, false, false, gate),
        )
        .await?;
    executor
        .wait_for_status(&source_id, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    // The source forwards its own intent; the counts below start after its receipt.
    let source_oplog = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
            if session_records(&oplog, is_cancel_applied).next().is_some() {
                break anyhow::Ok(oplog);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;
    let cut = session_records(&source_oplog, is_cancel_intent)
        .next()
        .expect("dropping the stream records a cancellation intent")
        .oplog_index;
    let source_records = |oplog: &[PublicOplogEntryWithIndex]| {
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::StreamSession(_)))
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    let source_before = source_records(&source_oplog);
    let controls_before = controls.load(Ordering::SeqCst);

    let fork = executor.fork_worker_never_live(&source_id, cut).await?;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 1, true).await?;
    assert_eq!(controls.load(Ordering::SeqCst), controls_before);

    drop(executor);
    let executor = start(deps, &context, &controls).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 2, true).await?;
    let fork_oplog = executor.get_oplog(&fork, OplogIndex::INITIAL).await?;
    assert_eq!(
        session_records(&fork_oplog, is_cancel_applied)
            .filter(|entry| entry.oplog_index > cut)
            .count(),
        0
    );
    assert!(
        update_outcomes(&executor, &fork)
            .await?
            .iter()
            .all(|outcome| matches!(outcome.entry, PublicOplogEntry::SuccessfulUpdate(_)))
    );

    executor.delete_worker(&fork).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(controls.load(Ordering::SeqCst), controls_before);
    assert_eq!(
        source_records(&executor.get_oplog(&source_id, OplogIndex::INITIAL).await?),
        source_before,
        "the fork's lifecycle changed the stream records of its source"
    );

    executor.fork_worker(&source_id, cut).await?;
    tokio::time::timeout(Duration::from_secs(60), async {
        while controls.load(Ordering::SeqCst) == controls_before {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the unmarked fork never forwarded the cancellation intent"))?;
    Ok(())
}
