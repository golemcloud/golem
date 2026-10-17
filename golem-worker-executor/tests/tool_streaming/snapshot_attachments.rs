use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn cancelled_stdin_disposal_replay_consumes_historical_finish(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::attachment::{ToolAttachmentControl, ToolAttachmentSlot};
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!("ToolStreamingCaller", "cancelled-stdin-disposal-replay");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let _: String = executor
        .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    let mut body = executor.gate_next_live_entity_body_completion(&worker, "streaming");
    let settled = executor.probe_primary_settlement_access(&worker);
    {
        let invocation = executor.invoke_and_await_agent(
            &component,
            &agent,
            "drop_cancelled_fire_and_forget_stdin",
            data_value!(),
        );
        tokio::pin!(invocation);
        let observe = async {
            tokio::time::timeout(Duration::from_secs(30), body.entered()).await?;
            tokio::time::timeout(Duration::from_secs(30), settled).await??;
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {
            result = &mut invocation => panic!("invocation escaped held entity completion: {result:?}"),
            result = observe => result?,
        }
        body.release();
        tokio::time::timeout(Duration::from_secs(30), invocation).await??;
    }
    let original = executor.stored_oplog(&worker).await;
    let controls = |entries: &[golem_common::model::oplog::OplogEntry]| {
        entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    golem_common::model::oplog::OplogEntry::ToolAttachmentControl { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(!controls(&original).iter().any(|entry| matches!(entry, golem_common::model::oplog::OplogEntry::ToolAttachmentControl { attachment, transition: ToolAttachmentControl::TerminalSelected(_), .. } if attachment.slot == ToolAttachmentSlot::Stdin)));
    let starts = |entries: &[golem_common::model::oplog::OplogEntry]| {
        entries
            .iter()
            .filter(|entry| matches!(entry, golem_common::model::oplog::OplogEntry::Start { .. }))
            .cloned()
            .collect::<Vec<_>>()
    };
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides).await?;
    let mut claim = executor.gate_next_entity_reconstruction_claim(&worker);
    let mut body = executor.gate_next_entity_body_start(&worker);
    {
        let invocation =
            executor.invoke_and_await_agent(&component, &agent, "replay_probe", data_value!());
        tokio::pin!(invocation);
        let observe = async {
            tokio::time::timeout(Duration::from_secs(30), claim.entered()).await?;
            let settled = executor.probe_primary_settlement_access(&worker);
            claim.release();
            tokio::time::timeout(Duration::from_secs(30), body.entered()).await?;
            // The replayed closed.wait and writer drop precede primary guest settlement,
            // even though the local consumer has not run yet.
            tokio::time::timeout(Duration::from_secs(30), settled).await??;
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {
            result = &mut invocation => panic!("replay escaped held entity body: {result:?}"),
            result = observe => result?,
        }
        body.release();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), invocation)
                .await??
                .into_typed::<String>()?,
            "replayed"
        );
    }
    let replayed = executor.stored_oplog(&worker).await;
    assert_eq!(controls(&replayed), controls(&original));
    assert_eq!(starts(&replayed), starts(&original));
    executor.delete_worker(&worker).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
async fn snapshot_attachment_constructors_and_reads_are_suppressed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in ["stdin", "output", "retained"] {
        let context = TestContext::new(last_unique_id);
        let mut overrides = TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.retry.max_attempts = 0;
                config.oplog.default_snapshotting =
                    golem_worker_executor::services::golem_config::SnapshotPolicy::EveryNInvocation { count: 1 };
            })),
            ..Default::default()
        };
        let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, caller)
            .store()
            .await?;
        let agent = agent_id!("ToolStreamingCaller", format!("snapshot-{mode}"));
        let worker = executor
            .start_agent_with(
                &component.id,
                agent.clone(),
                HashMap::from([("SNAPSHOT_ATTACHMENTS".to_string(), mode.to_string())]),
                Vec::new(),
            )
            .await?;
        let _: String = executor
            .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
            .await?
            .into_typed()?;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
                if entries
                    .iter()
                    .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
                {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        overrides.configure = Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        }));
        for _ in 0..2 {
            drop(executor);
            executor = start_with_overrides(deps, &context, overrides.clone()).await?;
            let mut events = executor.capture_output(&worker).await?;
            let method = if mode == "retained" {
                "wait_retained_stdin_closed"
            } else {
                "replay_probe"
            };
            let result: String = executor
                .invoke_and_await_agent(&component, &agent, method, data_value!())
                .await?
                .into_typed()?;
            crate::durability::assert_snapshot_recovery_loaded(&mut events).await;
            assert_eq!(
                result,
                if mode == "retained" {
                    "ByteStreamCloseCause::Finished"
                } else {
                    "replayed"
                }
            );
            let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            assert!(!entries.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(start) if start.function_name.starts_with("golem::tool::")
            )), "snapshot attachment operations must not append Start records");
            assert!(
                !entries
                    .iter()
                    .any(|entry| matches!(entry.entry, PublicOplogEntry::ToolAttachmentRead(_))),
                "snapshot reads must not append observations"
            );
        }
        executor.delete_worker(&worker).await?;
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn producer_disposal_receipt_blocks_invocation_success(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::attachment::{ToolAttachmentControl, ToolAttachmentSlot};

    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let agent = agent_id!("ToolStreamingCaller", "delayed-stdin-disposal-receipt");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let _: String = executor
        .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let finishes = |entries: &[PublicOplogEntryWithIndex]| {
        entries
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count()
    };
    let (entered, release) = executor.gate_next_stdin_terminal_receipt(&worker);
    {
        let invocation = executor.invoke_and_await_agent(
            &component,
            &agent,
            "retain_stdin_closed",
            data_value!(),
        );
        tokio::pin!(invocation);
        tokio::select! {
            result = &mut invocation => panic!("invocation returned before its disposal receipt gate: {result:?}"),
            entered = tokio::time::timeout(Duration::from_secs(30), entered) => { entered??; }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut invocation)
                .await
                .is_err(),
            "invocation success must wait for the disposal receipt"
        );
        let held = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        assert_eq!(
            finishes(&held),
            finishes(&before),
            "Finished must not precede disposal publication"
        );
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("disposal receipt waiter disappeared"))?;
        invocation.await?;
    }
    let expected = "ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)";
    let closed: String = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "wait_retained_stdin_closed",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(closed, expected);
    let terminal_controls = |entries: &[golem_common::model::oplog::OplogEntry]| {
        entries
            .iter()
            .filter_map(|entry| match entry {
                golem_common::model::oplog::OplogEntry::ToolAttachmentControl {
                    attachment,
                    transition: ToolAttachmentControl::TerminalSelected(terminal),
                    ..
                } if attachment.slot == ToolAttachmentSlot::Stdin => {
                    Some((*attachment, terminal.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let original = executor.stored_oplog(&worker).await;
    let terminals = terminal_controls(&original);
    assert_eq!(
        terminals.len(),
        1,
        "writer abandonment selects exactly one terminal"
    );
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides).await?;
    let closed: String = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "wait_retained_stdin_closed",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(closed, expected);
    let probe: String = executor
        .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(probe, "replayed");
    assert_eq!(
        terminal_controls(&executor.stored_oplog(&worker).await),
        terminals,
        "completed replay must preserve the single disposal terminal"
    );
    executor.delete_worker(&worker).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
async fn producer_disposal_receipt_blocks_closed_notification(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let agent = agent_id!("ToolStreamingCaller", "delayed-stdin-closed-notification");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let _: String = executor
        .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    let method = "abandon_stdin_and_wait_closed";
    let (entered, release) = executor.gate_next_stdin_terminal_receipt(&worker);
    let mut success = executor.gate_next_agent_function_success(&worker, method);
    {
        let invocation = executor.invoke_and_await_agent(&component, &agent, method, data_value!());
        tokio::pin!(invocation);
        tokio::select! {
            result = &mut invocation => panic!("closed wait returned before its disposal receipt gate: {result:?}"),
            entered = tokio::time::timeout(Duration::from_secs(30), entered) => { entered??; }
        }
        // This gate precedes the production success drain. Reaching it would mean that the
        // guest observed closure even though the disposal receipt is still held.
        assert!(
            tokio::time::timeout(Duration::from_millis(250), success.entered())
                .await
                .is_err(),
            "closed notification must wait for disposal publication"
        );
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("disposal receipt waiter disappeared"))?;
        tokio::time::timeout(Duration::from_secs(30), success.entered()).await?;
        success.release();
        assert_eq!(
            invocation.await?.into_typed::<String>()?,
            "ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)"
        );
    }
    let terminal_count = |entries: &[golem_common::model::oplog::OplogEntry]| {
        entries.iter().filter(|entry| matches!(entry,
        golem_common::model::oplog::OplogEntry::ToolAttachmentControl { transition: golem_common::model::oplog::attachment::ToolAttachmentControl::TerminalSelected(_), .. }
    )).count()
    };
    assert_eq!(terminal_count(&executor.stored_oplog(&worker).await), 1);
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides).await?;
    let probe: String = executor
        .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(probe, "replayed");
    assert_eq!(terminal_count(&executor.stored_oplog(&worker).await), 1);
    executor.delete_worker(&worker).await?;
    Ok(())
}
