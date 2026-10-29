//! A real executor restart selects a different disposal schedule through snapshot load.
use super::*;
use golem_common::model::AgentEvent;
use golem_common::schema::IntoSchema;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);

fn overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
            config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
        })),
        ..Default::default()
    }
}

struct Scenario {
    live: u32,
    replay: u32,
    reads: u32,
    unwrap: bool,
    omit_read: bool,
}

async fn scenario(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    fixture: &PrecompiledComponent,
    Scenario {
        live,
        replay,
        reads,
        unwrap,
        omit_read,
    }: Scenario,
) -> anyhow::Result<()> {
    let executor = start_with_overrides(deps, context, overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = format!("input-{live}-{replay}-{reads}-{omit_read}");
    let target = executor
        .start_agent(&component.id, agent_id!("StreamingRpcTarget", name.clone()))
        .await?;
    let id = agent_id!("InputGc", name);
    let agent = executor.start_agent(&component.id, id.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &id, "configure", data_value!(live, replay))
        .await?;
    let gate: PromiseId = executor
        .invoke_and_await_agent(&component, &id, "promise", data_value!())
        .await?
        .into_typed()?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &id,
            &key,
            "run",
            data_value!(reads, unwrap, omit_read, gate.clone()),
        )
        .await?;
    executor
        .wait_for_status(&agent, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    let cancel_intents = |history: &[PublicOplogEntryWithIndex]| {
        history
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                    StreamSessionRecord::ConsumerCancelIntent(_)))
            })
            .count()
    };
    assert_eq!(cancel_intents(&history), usize::from(live != 2));
    let consumed = history
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
            if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                StreamSessionRecord::ConsumerItemValue(_)))
        })
        .count();
    assert_eq!(
        consumed, reads as usize,
        "the crash follows exactly the requested completed reads"
    );
    let snapshot = history
        .iter()
        .rev()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .expect("snapshot before stream invocation");
    let run = history
        .iter()
        .rev()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .unwrap();
    assert!(snapshot.oplog_index < run.oplog_index);
    drop(executor);
    let executor = start_with_overrides(deps, context, overrides()).await?;
    let mut events = executor.capture_output(&agent).await?;
    executor.complete_promise(&gate, vec![]).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &id,
            &key,
            "run",
            data_value!(reads, unwrap, omit_read, gate),
        ),
    )
    .await?;
    if omit_read {
        // Automatic snapshots are disposable baselines. Strict divergence rejects the
        // changed snapshot and retries authoritative history, not the changed read sequence.
        result?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = events.recv().await {
                if let Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) =
                    AgentEvent::try_from(event)
                {
                    assert!(error.contains("Unexpected oplog entry"), "{error}");
                    return;
                }
            }
            panic!("omitting a completed read must reject the snapshot");
        })
        .await?;
        return Ok(());
    }
    result?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut loaded = false;
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                    panic!("disposal-only changes must not reject a snapshot: {error}");
                }
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    assert_eq!(snapshot_index, snapshot.oplog_index);
                    loaded = true;
                }
                Ok(AgentEvent::InvocationFinished { function, .. }) if function == "run" => {
                    assert!(loaded);
                    return;
                }
                _ => {}
            }
        }
        panic!("missing snapshot recovery result");
    })
    .await?;
    let report: (bool, u32, u32) = executor
        .invoke_and_await_agent(&component, &id, "inspect", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(report, (true, reads, u32::from(replay == 2)));
    let values: Vec<u32> = executor
        .invoke_and_await_agent(&component, &id, "continuation", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(values, vec![30, 31, 32]);
    executor.commit_oplog(&target).await?;
    let target_history = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    assert_eq!(
        target_history
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .count(),
        3,
        "constructor, one original production and one fresh continuation, with no repeated target invocation"
    );
    executor.commit_oplog(&agent).await?;
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    for entry in &history {
        if let PublicOplogEntry::Start(_) = &entry.entry {
            assert!(
                history.iter().any(|terminal| match &terminal.entry {
                    PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                    PublicOplogEntry::Cancelled(cancel) => cancel.start_index == entry.oplog_index,
                    _ => false,
                }),
                "unsettled Start at {}",
                entry.oplog_index
            );
        }
    }
    let finished = history
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .unwrap()
        .oplog_index;
    assert!(!history.iter().any(|entry| entry.oplog_index > finished
        && matches!(
            entry.entry,
            PublicOplogEntry::Start(_) | PublicOplogEntry::End(_) | PublicOplogEntry::Cancelled(_)
        )));
    Ok(())
}

#[test]
#[timeout("5 minutes")]
async fn unread_stream_disposal_schedule_switch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (live, replay) in [(2, 0), (1, 0), (0, 1), (0, 2)] {
        scenario(
            deps,
            &TestContext::new(last_unique_id),
            fixture,
            Scenario {
                live,
                replay,
                reads: 0,
                unwrap: false,
                omit_read: false,
            },
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("5 minutes")]
async fn partial_read_preserved_with_different_disposal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (live, replay) in [(2, 0), (1, 0), (0, 1), (0, 2)] {
        scenario(
            deps,
            &TestContext::new(last_unique_id),
            fixture,
            Scenario {
                live,
                replay,
                reads: 1,
                unwrap: false,
                omit_read: false,
            },
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn disposal_before_a_recorded_read_remains_divergence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    scenario(
        deps,
        &TestContext::new(last_unique_id),
        fixture,
        Scenario {
            live: 2,
            replay: 0,
            reads: 1,
            unwrap: false,
            omit_read: true,
        },
    )
    .await
}

#[test]
#[timeout("5 minutes")]
async fn unwrapped_unread_stream_disposal_schedule_switch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (live, replay) in [(2, 0), (1, 0), (0, 1), (0, 2)] {
        scenario(
            deps,
            &TestContext::new(last_unique_id),
            fixture,
            Scenario {
                live,
                replay,
                reads: 0,
                unwrap: true,
                omit_read: false,
            },
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, FromSchema)]
struct TsInputGcReport {
    restored: bool,
    reads: f64,
    retained: f64,
}

#[derive(Clone, FromSchema, IntoSchema)]
#[schema(rename_all = "camelCase")]
struct TsInputGcPromise {
    agent_id: TsInputGcAgentId,
    oplog_idx: u64,
}

#[derive(Clone, FromSchema, IntoSchema)]
#[schema(rename_all = "camelCase")]
struct TsInputGcAgentId {
    component_id: TsInputGcComponentId,
    agent_id: String,
}

#[derive(Clone, FromSchema, IntoSchema)]
struct TsInputGcComponentId {
    uuid: TsInputGcUuid,
}

#[derive(Clone, FromSchema, IntoSchema)]
#[schema(rename_all = "camelCase")]
struct TsInputGcUuid {
    high_bits: u64,
    low_bits: u64,
}

#[test]
#[timeout("2 minutes")]
async fn completed_invocation_drop_has_no_following_guest_host_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = || TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 2 };
            config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, config()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let id = agent_id!("InputGc", "completed-drop");
    let agent = executor.start_agent(&component.id, id.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &id, "configure", data_value!(2_u32, 0_u32))
        .await?;
    executor
        .invoke_and_await_agent(&component, &id, "run_to_end", data_value!())
        .await?;
    executor.commit_oplog(&agent).await?;
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    let snapshot = history
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
        .unwrap()
        .oplog_index;
    let starts = history
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert!(
        snapshot < *starts.last().unwrap(),
        "the completed stream invocation must replay"
    );
    assert!(!history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
        if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(), StreamSessionRecord::ConsumerCancelIntent(_)))));
    drop(executor);
    let executor = start_with_overrides(deps, &context, config()).await?;
    let mut events = executor.capture_output(&agent).await?;
    let report: (bool, u32, u32) = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&component, &id, "inspect", data_value!()),
    )
    .await??
    .into_typed()?;
    assert_eq!(report, (true, 0, 0));
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut loaded = false;
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => panic!("{error}"),
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    assert_eq!(snapshot_index, snapshot);
                    loaded = true;
                }
                Ok(AgentEvent::InvocationFinished { function, .. }) if function == "inspect" => {
                    assert!(loaded);
                    return;
                }
                _ => {}
            }
        }
        panic!("missing snapshot recovery result");
    })
    .await?;
    let values: Vec<u32> = executor
        .invoke_and_await_agent(&component, &id, "continuation", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(values, vec![30, 31, 32]);
    Ok(())
}

#[test]
#[timeout("5 minutes")]
async fn typescript_gc_collects_previously_retained_input_with_identical_reads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for reads in [0_f64, 1_f64] {
        let context = TestContext::new(last_unique_id);
        let executor = start_with_overrides(deps, &context, overrides()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let id = agent_id!("TsInputGc", format!("ts-input-{reads}"));
        let agent = executor.start_agent(&component.id, id.clone()).await?;
        let gate: TsInputGcPromise = executor
            .invoke_and_await_agent(&component, &id, "promise", data_value!())
            .await?
            .into_typed()?;
        let key = IdempotencyKey::fresh();
        executor
            .invoke_agent_with_key(
                &component,
                &id,
                &key,
                "run",
                data_value!(reads, gate.clone()),
            )
            .await?;
        executor
            .wait_for_status(&agent, AgentStatus::Suspended, Duration::from_secs(30))
            .await?;
        let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
        assert!(
            history
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
        );
        assert_eq!(
            history
                .iter()
                .filter(
                    |entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                    if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                        StreamSessionRecord::ConsumerItemValue(_)))
                )
                .count(),
            reads as usize,
            "GC and reconstruction must preserve the completed read sequence"
        );
        assert!(!history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
            if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(), StreamSessionRecord::ConsumerCancelIntent(_)))));
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides()).await?;
        let mut events = executor.capture_output(&agent).await?;
        executor
            .complete_promise(
                &PromiseId {
                    agent_id: agent.clone(),
                    oplog_idx: OplogIndex::from_u64(gate.oplog_idx),
                },
                vec![],
            )
            .await?;
        tokio::time::timeout(
            Duration::from_secs(30),
            executor.invoke_and_await_agent_with_key(
                &component,
                &id,
                &key,
                "run",
                data_value!(reads, gate),
            ),
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut loaded = false;
            while let Some(event) = events.recv().await {
                match AgentEvent::try_from(event) {
                    Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                        panic!("GC must not reject the snapshot: {error}")
                    }
                    Ok(AgentEvent::SnapshotRecoverySucceeded { .. }) => loaded = true,
                    Ok(AgentEvent::InvocationFinished { function, .. }) if function == "run" => {
                        assert!(loaded);
                        return;
                    }
                    _ => {}
                }
            }
            panic!("missing snapshot recovery result");
        })
        .await?;
        let report: TsInputGcReport = executor
            .invoke_and_await_agent(&component, &id, "inspect", data_value!())
            .await?
            .into_typed()?;
        assert!(report.restored);
        assert_eq!(report.reads, reads);
        assert_eq!(report.retained, 0.0);
        executor.commit_oplog(&agent).await?;
        let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
        assert!(
            history.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::StreamSession(session)
                if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                    StreamSessionRecord::ConsumerCancelIntent(_)))),
            "GC must actually release the input, not merely stop retaining its wrapper"
        );
        let values: Vec<u32> = executor
            .invoke_and_await_agent(&component, &id, "continuation", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(values, vec![30, 31, 32]);
    }
    Ok(())
}
