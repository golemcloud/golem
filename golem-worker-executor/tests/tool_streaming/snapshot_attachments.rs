use super::*;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use test_r::test;

pub(super) struct DisposalScheduleComponentService {
    pub(super) inner: Arc<dyn golem_worker_executor::services::component::ComponentService>,
    pub(super) bytes: Arc<Vec<u8>>,
    pub(super) schedule: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl golem_worker_executor::services::component::ComponentService
    for DisposalScheduleComponentService
{
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        id: golem_common::model::component::ComponentId,
        revision: ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        WorkerExecutorError,
    > {
        let metadata = self.inner.get_metadata(id, Some(revision)).await?;
        if metadata.component_name.0 != "gc-disposal-provider" {
            return self.inner.get(engine, id, revision).await;
        }
        let marker = b"tool-gc-schedule-marker:0";
        let mut bytes = self.bytes.as_ref().clone();
        let positions = bytes
            .windows(marker.len())
            .enumerate()
            .filter_map(|(index, window)| (window == marker).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(
            positions.len(),
            1,
            "the fixture must retain exactly one schedule data marker"
        );
        bytes[positions[0] + marker.len() - 1] = b'0' + self.schedule.load(Ordering::SeqCst) as u8;
        let component = wasmtime::component::Component::new(engine, bytes)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok((component, metadata))
    }

    async fn get_metadata(
        &self,
        id: golem_common::model::component::ComponentId,
        revision: Option<ComponentRevision>,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        self.inner.get_metadata(id, revision).await
    }

    async fn resolve_component(
        &self,
        reference: String,
        environment: golem_common::model::environment::EnvironmentId,
        application: golem_common::model::application::ApplicationId,
        account: AccountId,
    ) -> Result<Option<golem_common::model::component::ComponentId>, WorkerExecutorError> {
        self.inner
            .resolve_component(reference, environment, application, account)
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        self.inner.all_cached_metadata().await
    }

    async fn invalidate_all_metadata_for_environment(
        &self,
        environment: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_all_metadata_for_environment(environment)
            .await
    }
}

#[test]
#[timeout("10m")]
async fn completed_entity_writer_disposal_replays_its_attachment_terminals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::attachment::{
        ByteStreamCloseCause, ByteStreamFailure, ToolAttachmentControl, ToolAttachmentSlot,
        ToolAttachmentTerminal,
    };

    let bytes = Arc::new(
        tokio::fs::read(
            deps.component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
        )
        .await?,
    );
    for (live_schedule, replay_schedule) in [(0, 1), (1, 0), (2, 0), (0, 2)] {
        let schedule = Arc::new(AtomicUsize::new(live_schedule));
        let selected_schedule = schedule.clone();
        let provider_bytes = bytes.clone();
        let context = TestContext::new(last_unique_id);
        let environment_state = Arc::new(TestEnvironmentStateService::default());
        let overrides = TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(DisposalScheduleComponentService {
                    inner,
                    bytes: provider_bytes.clone(),
                    schedule: selected_schedule.clone(),
                })
            })),
            configure: Some(Arc::new(|config| {
                config.oplog.default_snapshotting =
                    golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        };
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let provider_component = executor
            .component_dep(&context.default_environment_id, provider)
            .name("gc-disposal-provider")
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
        let agent = agent_id!("ToolStreamingCaller", "completed-entity-writer-disposal");
        let worker = executor.start_agent(&component.id, agent.clone()).await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "gol40_rust_sdk_terminal_observation",
                data_value!(),
            )
            .await?;
        let before = executor.stored_oplog(&worker).await;
        let creations = before
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                matches!(
                    entry,
                    OplogEntry::Start {
                        function_name:
                            golem_common::model::oplog::host_functions::HostFunctionName::GolemToolCreateOutput,
                        parent_start_index: Some(_),
                        ..
                    }
                )
                .then_some(OplogIndex::from_u64(index as u64 + 1))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            creations.len(),
            1,
            "the provider creates one unreachable output"
        );
        let terminal = before.iter().find_map(|entry| match entry {
            OplogEntry::ToolAttachmentControl {
                attachment,
                transition: ToolAttachmentControl::TerminalSelected(terminal),
                ..
            } if attachment.creation_start == creations[0]
                && attachment.slot == ToolAttachmentSlot::Output =>
            {
                Some(terminal)
            }
            _ => None,
        });
        if live_schedule != 2 {
            assert_eq!(
                terminal,
                Some(&ToolAttachmentTerminal::Application(
                    ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)
                ))
            );
        } else {
            assert_eq!(
                terminal, None,
                "neither unreachable endpoint is collected before Store teardown"
            );
        }
        let controls = |history: &[OplogEntry]| {
            history
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::ToolAttachmentControl { .. }))
                .cloned()
                .collect::<Vec<_>>()
        };
        drop(executor);
        schedule.store(replay_schedule, Ordering::SeqCst);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let probe: String = executor
            .invoke_and_await_agent(&component, &agent, "replay_probe", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(probe, "replayed");
        let after = executor.stored_oplog(&worker).await;
        assert_eq!(&after[..before.len()], before.as_slice());
        assert_eq!(
            controls(&after),
            controls(&before),
            "completed entity disposal cannot replace or duplicate a terminal"
        );
        executor.delete_worker(&worker).await?;
    }
    Ok(())
}

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
    assert_eq!(starts(&replayed[..original.len()]), starts(&original));
    // The fresh probe performs the SDK's per-invocation identity lookup. Reconstruction
    // must not add any other host call, including another stdin disposal operation.
    assert!(matches!(
        starts(&replayed[original.len()..]).as_slice(),
        [golem_common::model::oplog::OplogEntry::Start {
            function_name: golem_common::model::oplog::payload::host_functions::HostFunctionName::WasiCliEnvironmentGetEnvironment,
            ..
        }]
    ));
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
