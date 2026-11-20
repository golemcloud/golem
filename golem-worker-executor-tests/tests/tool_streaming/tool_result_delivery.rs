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

//! A tool result is an async guest event. The guest receives it only at a blocked wait, and the
//! oplog records that boundary, so a rebuild releases the result at the same point.

use super::*;
use golem_common::model::durable_stream::{StreamItemsPayload, StreamItemsRecord};
use golem_worker_executor::services::resource_limits::AtomicResourceEntry;
use golem_worker_executor_test_utils::SharedResourceLimits;
use prost::Message;
use test_r::test;

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn malformed_raw_tool_return_does_not_resume_guest_tail(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    raw_tool_return_boundary(
        last_unique_id,
        deps,
        provider,
        caller,
        RawReturnBoundary::Clock,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn raw_tool_return_records_http_admission_without_dispatch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for denied in [false, true] {
        raw_tool_return_boundary(
            last_unique_id,
            deps,
            provider,
            caller,
            RawReturnBoundary::Http { denied },
        )
        .await?;
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn raw_tool_return_preserves_a_held_start_across_shutdown(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for shutdown in [false, true] {
        raw_tool_return_boundary(
            last_unique_id,
            deps,
            provider,
            caller,
            RawReturnBoundary::HeldStart { shutdown },
        )
        .await?;
    }
    Ok(())
}

enum RawReturnBoundary {
    Clock,
    Http { denied: bool },
    HeldStart { shutdown: bool },
}

async fn raw_tool_return_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    boundary: RawReturnBoundary,
) -> anyhow::Result<()> {
    let http = matches!(boundary, RawReturnBoundary::Http { .. });
    let denied = matches!(boundary, RawReturnBoundary::Http { denied: true });
    let shutdown = matches!(boundary, RawReturnBoundary::HeldStart { shutdown: true });
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let authority = listener.local_addr()?.to_string();
    let app = Router::new().route(
        "/checkpoint",
        get(move || async move {
            observed.fetch_add(1, Ordering::SeqCst);
            "unexpected request"
        }),
    );
    let mut server = tokio::task::JoinSet::new();
    server.spawn(async move { axum::serve(listener, app).await });
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let limits = Arc::new(AtomicResourceEntry::new_with_all_limits(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        7,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    ));
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        resource_limits: Some(Arc::new(SharedResourceLimits(limits.clone()))),
        configure: Some(Arc::new(|config| {
            // Observe one returned Store, not a later incomplete-reconstruction attempt.
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .update_agent_provision_config("ToolStreamingCaller", |config| {
            if denied {
                config.initial_permissions.lower_bound.negative.push(
                    golem_common::model::card::parse_polymorphic_permission(
                        "network() @ * : connect : *",
                    )
                    .unwrap(),
                );
            }
        })
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
    let definition = metadata
        .tools
        .iter()
        .find(|tool| {
            tool.commands
                .nodes
                .first()
                .is_some_and(|root| root.name == "return-boundary-probe")
        })
        .expect("raw return probe is exported")
        .clone();
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let commands: &[&str] = if matches!(boundary, RawReturnBoundary::HeldStart { .. }) {
        &["unpadded"]
    } else if http {
        &["pending-http"]
    } else {
        &["valid", "malformed", "unpadded"]
    };
    for &command in commands {
        let agent = agent_id!("ToolStreamingCaller", format!("raw-return-{command}"));
        let worker = executor
            .start_agent(&caller_component.id, agent.clone())
            .await?;
        let fingerprint = executor.get_worker_metadata(&worker).await?.fingerprint;
        let mut disposals = executor.probe_entity_store_disposal(&worker);
        let path = vec![command.to_string()];
        let index = definition
            .command_index_by_path(&path)
            .expect("probe command");
        let schema = definition.canonical_input_record_schema(index)?;
        let mut body_gate = http.then(|| executor.gate_next_entity_body_start(&worker));
        let mut start_gate = if matches!(boundary, RawReturnBoundary::HeldStart { .. }) {
            Some(executor.gate_after_next_append(&worker, |entry| {
                matches!(entry, golem_common::model::oplog::OplogEntry::Start { function_name, .. }
                    if function_name.to_string().ends_with("monotonic-clock::wait-for"))
            }).await)
        } else {
            None
        };
        let owner = executor
            .active_agent(&OwnedAgentId::new(context.default_environment_id, &worker))
            .await
            .expect("the probe owner is resident");
        let held_authority = async {
            if let Some(gate) = &mut body_gate {
                gate.entered().await;
                let mut append = executor
                    .gate_after_next_append(&worker, |entry| {
                        matches!(
                            entry,
                            golem_common::model::oplog::OplogEntry::CardEventQueued { .. }
                        )
                    })
                    .await;
                let primary = owner.primary();
                let revocation =
                    primary.queue_card_revocation(golem_common::model::card::CardId::new());
                let release = async {
                    append.entered().await;
                    gate.release();
                    // Hold the real authority boundary while the raw export issues its import
                    // and returns. This is a validation pause, not padding in the guest fixture.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    assert_eq!(limits.remaining_http_calls(), 7);
                    assert_eq!(requests.load(Ordering::SeqCst), 0);
                    drop(append);
                };
                let (revoked, ()) = tokio::join!(revocation, release);
                revoked?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let held_start = async {
            if let Some(gate) = &mut start_gate {
                gate.entered().await;
                let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
                let parent = entries
                    .iter()
                    .find_map(|entry| {
                        matches!(&entry.entry, PublicOplogEntry::Start(start)
                        if start.function_name == "golem::entity::invoke")
                        .then_some(entry.oplog_index)
                    })
                    .expect("the tool parent must precede its import");
                assert!(
                    !entries.iter().any(|entry| matches!(&entry.entry,
                    PublicOplogEntry::End(end) if end.start_index == parent)),
                    "a held Start receipt must prevent parent completion"
                );
                if shutdown {
                    gate.abort();
                    executor.shutdown_and_wait_for_invocation_loops().await?;
                }
            }
            drop(start_gate.take());
            Ok::<_, anyhow::Error>(())
        };
        let invocation = tokio::time::timeout(
            Duration::from_secs(20),
            executor.invoke_external_tool(
                &worker,
                fingerprint,
                IdempotencyKey::fresh(),
                ToolName::try_from("return-boundary-probe").unwrap(),
                path,
                TypedSchemaValue::new(
                    schema,
                    SchemaValue::Record {
                        fields: if http {
                            vec![SchemaValue::String(authority.clone())]
                        } else {
                            vec![]
                        },
                    },
                ),
                InvocationContextStack::fresh(),
                Principal::GolemUser(GolemUserPrincipal {
                    account_id: context.account_id,
                }),
                None,
            ),
        );
        let (result, (), ()) = tokio::try_join!(
            async { Ok::<_, anyhow::Error>(invocation.await?) },
            held_authority,
            held_start
        )?;
        if shutdown {
            assert!(
                result.is_err(),
                "shutdown must not publish a successful tool result"
            );
        } else if command != "malformed" {
            let AgentInvocationResult::ExternalTool { result: Ok(result) } = result?.result else {
                panic!("valid raw return must succeed");
            };
            assert_eq!(
                result.result.unwrap().value(),
                &SchemaValue::String("raw-return-control".into())
            );
        } else {
            let error = result.expect_err("host decoding must reject the raw payload");
            assert!(
                format!("{error:?}").contains("tool guest returned an invalid result payload"),
                "{error:?}"
            );
        }
        if command == "unpadded" || http {
            let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            let function = if http {
                "http::client::send"
            } else {
                "monotonic-clock::wait-for"
            };
            let starts = before
                .iter()
                .filter_map(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name.ends_with(function))
                    .then_some(entry.oplog_index)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                starts.len(),
                1,
                "the import issued immediately before return must have a Start: {function}"
            );
            assert_eq!(
                requests.load(Ordering::SeqCst),
                0,
                "admission must not dispatch the HTTP effect after return"
            );
            if http {
                assert!(
                    before.iter().any(|entry| matches!(&entry.entry,
                    PublicOplogEntry::Start(start) if start.function_name.ends_with(function)
                        && start.denial.is_some() == denied)),
                    "the Start must retain the permission decision"
                );
                assert_eq!(
                    limits.remaining_http_calls(),
                    if denied { 7 } else { 6 },
                    "only authorized admission charges an HTTP call"
                );
            }
            assert!(!before.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(start) if start.function_name == "golem::api::create_promise")),
                "the guest tail must not execute");
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            let replacement = start_with_overrides(deps, &context, overrides).await?;
            let replayed: String = replacement
                .invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!())
                .await?
                .into_typed()?;
            assert_eq!(replayed, "replayed");
            let replayed = replacement.get_oplog(&worker, OplogIndex::INITIAL).await?;
            let replayed_starts = replayed
                .iter()
                .filter_map(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name.ends_with(function))
                    .then_some(entry.oplog_index)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                replayed_starts, starts,
                "reconstruction must claim the original Start"
            );
            if !http {
                assert!(
                    !replayed.iter().any(|entry| match &entry.entry {
                        PublicOplogEntry::End(end) => starts.contains(&end.start_index),
                        PublicOplogEntry::Cancelled(cancelled) =>
                            starts.contains(&cancelled.start_index),
                        _ => false,
                    }),
                    "reconstruction must not manufacture a terminal for the pending timer"
                );
            }
            assert!(
                !replayed
                    .iter()
                    .any(|entry| matches!(&entry.entry, PublicOplogEntry::Error(_))),
                "completed reconstruction must preserve the unpadded import"
            );
            assert_eq!(
                requests.load(Ordering::SeqCst),
                0,
                "completed replay must not dispatch the HTTP effect"
            );
            if http {
                assert_eq!(
                    limits.remaining_http_calls(),
                    if denied { 7 } else { 6 },
                    "replay must not charge admission twice"
                );
            }
            return Ok(());
        }
        // Disposal drops the Store-bound timer and the only guest that could resume its tail.
        let disposed = tokio::time::timeout(Duration::from_secs(5), disposals.recv()).await?;
        let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let entity_starts: Vec<_> = entries.iter().filter_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke")
                .then_some(entry.oplog_index)
        }).collect();
        assert_eq!(entity_starts.len(), 1);
        assert_eq!(
            disposed,
            Some(entity_starts[0]),
            "the returned Store must be destroyed while the executor remains alive: {command}"
        );
        let timers: Vec<_> = entries.iter().filter_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name.ends_with("monotonic-clock::wait-for"))
                .then_some(entry.oplog_index)
        }).collect();
        assert_eq!(
            timers.len(),
            1,
            "the pending tail must have started: {command}"
        );
        assert!(
            !entries.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => end.start_index == timers[0],
                PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == timers[0],
                _ => false,
            }),
            "the returned timer must remain incomplete: {command}"
        );
        assert!(
            !entries.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::Start(start) if start.function_name == "golem::api::create_promise")),
            "the pending guest tail must not resume after raw return: {command}"
        );
    }
    Ok(())
}

fn recorded_string_items(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<Vec<String>> {
    let mut items = Vec::new();
    for entry in entries {
        if let PublicOplogEntry::StreamItems(params) = &entry.entry {
            let record =
                StreamItemsRecord::from_value(params.record.value()).map_err(anyhow::Error::msg)?;
            if let StreamItemsPayload::Values(values) = record.payload {
                for encoded in values {
                    let value = golem_schema::proto::golem::schema::SchemaValue::decode(
                        encoded.as_slice(),
                    )?;
                    if let Some(
                        golem_schema::proto::golem::schema::schema_value::Value::StringValue(value),
                    ) = value.value
                    {
                        items.push(value);
                    }
                }
            }
        }
    }
    Ok(items)
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn tool_return_stops_guest_tails_and_reconstructs_incomplete_imports(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
            config.limits.tail_work_settle_timeout = Duration::from_millis(500);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
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
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!("ToolStreamingCaller", "return-cutoff");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    // The generated Rust wrapper finishes stdout before export publication,
    // even when a spawned task retains a clone of the writer.
    for (mode, terminal) in [
        ("late", "finished"),
        ("infinite", "finished"),
        ("unused-live", "finished"),
        ("finished", "finished"),
        (
            "failed",
            "failed:ByteStreamFailure::Failed(\"stream producer failed\")",
        ),
    ] {
        let evidence: RedactionEvidence = tokio::time::timeout(
            Duration::from_secs(20),
            executor.invoke_and_await_agent(
                &caller_component,
                &agent,
                "redaction_stream_case",
                data_value!(format!("return-cutoff-{mode}")),
            ),
        )
        .await??
        .into_typed()?;
        assert_eq!(evidence.output, b"marker:", "{mode}");
        assert_eq!(evidence.stdout_terminal, terminal, "{mode}");
        assert_eq!(evidence.outcome, "ok", "{mode}");
    }
    let evidence: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "collect_capable",
            data_value!("return-cutoff-file:/return-cutoff", vec![17_u8, 201, 0, 43]),
        )
        .await?
        .into_typed()?;
    assert_evidence(&evidence, &[17, 201, 0, 43], 1, 4);
    let contents: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "read_owner_file",
            data_value!("/return-cutoff"),
        )
        .await?
        .into_typed()?;
    assert_eq!(contents, "before");
    assert!(
        executor
            .get_file_contents(&worker, "/return-cutoff-late")
            .await
            .is_err()
    );
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let timers: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name.ends_with("monotonic-clock::wait-for") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        timers.len(),
        2,
        "both tails must enter a timer import before return"
    );
    for timer in &timers {
        assert!(
            !entries.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => end.start_index == *timer,
                PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == *timer,
                _ => false,
            }),
            "a return cutoff must leave its ordinary timer incomplete"
        );
    }
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed: String = executor
        .invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(replayed, "replayed");
    let contents: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "read_owner_file",
            data_value!("/return-cutoff"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        contents, "before",
        "reconstruction must not run the file-writing suffix"
    );
    assert!(
        executor
            .get_file_contents(&worker, "/return-cutoff-late")
            .await
            .is_err()
    );
    let replayed = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    for timer in timers {
        assert!(
            !replayed.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => end.start_index == timer,
                PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == timer,
                _ => false,
            }),
            "completed reconstruction must not repair or cancel a cutoff timer"
        );
    }
    assert!(
        !replayed
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::Error(_)))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn output_producer_can_await_tools_live_and_after_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
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
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let collector = agent_id!("ToolStreamingCaller", "output-tool-collector");
    let mut targets = Vec::new();
    for capable in [false, true] {
        let name = format!("output-tool-{capable}");
        let agent = agent_id!("ToolStreamingCaller", name.clone());
        let worker = executor
            .start_agent(&caller_component.id, agent.clone())
            .await?;
        let gate = if capable {
            Some(
                executor
                    .invoke_and_await_agent(
                        &caller_component,
                        &agent,
                        "native_veto_gate",
                        data_value!(),
                    )
                    .await?
                    .into_typed::<PromiseId>()?,
            )
        } else {
            None
        };
        let checkpoint = executor.oplog_max_index(&worker).await?;
        let collect = executor.invoke_and_await_agent(
            &caller_component,
            &collector,
            "collect_tool_results",
            data_value!(name, capable, gate.clone()),
        );
        let crash = async {
            if let Some(gate) = gate {
                tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        let oplog = executor.get_oplog(&worker, checkpoint).await?;
                        let items = recorded_string_items(&oplog)?;
                        if items == ["before-tool", "first"] {
                            // The first capable tool has completed its filesystem write;
                            // the promise prevents admission of the second tool.
                            return Ok::<_, anyhow::Error>(());
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .map_err(|_| anyhow::anyhow!("producer did not publish its first tool result"))??;
                executor.simulated_crash(&worker).await?;
                executor.complete_promise(&gate, Vec::new()).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (output, crashed) = tokio::join!(collect, crash);
        crashed?;
        let output: Vec<String> = output?.into_typed()?;
        assert_eq!(output, ["before-tool", "first", "second"]);
        if capable {
            let order: String = executor
                .invoke_and_await_agent(
                    &caller_component,
                    &agent,
                    "read_owner_file",
                    data_value!("/capable-order.log".to_string()),
                )
                .await?
                .into_typed()?;
            assert_eq!(
                order, "firstsecond",
                "mid-invocation recovery duplicated or reordered tool writes"
            );
        }
        targets.push((agent, capable));
    }
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    for (agent, capable) in targets {
        let result: String = executor
            .invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(result, "replayed");
        if capable {
            let order: String = executor
                .invoke_and_await_agent(
                    &caller_component,
                    &agent,
                    "read_owner_file",
                    data_value!("/capable-order.log".to_string()),
                )
                .await?
                .into_typed()?;
            assert_eq!(
                order, "firstsecond",
                "reconstruction duplicated or reordered tool writes"
            );
        }
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn output_producer_trap_and_deadline_fail_the_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
    use golem_api_grpc::proto::golem::worker::{
        AgentInvocationMode, InvocationRequest, InvocationStart, invocation_request,
        invocation_response, invocation_session_completion,
    };

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.limits.max_invocation_duration = Some(Duration::from_secs(2));
                config.retry.max_attempts = 1;
                config.suspend.suspend_after = Duration::from_secs(120);
                config.suspend.wait_suspend_grace = Duration::from_secs(120);
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    for mode in ["trap", "deadline", "complete"] {
        let agent = agent_id!("ToolStreamingCaller", format!("producer-{mode}"));
        let worker = executor.start_agent(&component.id, agent.clone()).await?;
        let gate = executor
            .invoke_and_await_agent(&component, &agent, "native_veto_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        let metadata = executor.get_worker_metadata(&worker).await?;
        let checkpoint = executor.oplog_max_index(&worker).await?;
        let (_, input) = data_value!(mode.to_string(), gate.clone()).into_parts();
        let key = IdempotencyKey::fresh();
        let request = InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(worker.clone().into()),
                method_name: Some("produce_terminal".to_string()),
                input: Some(input.try_into().map_err(anyhow::Error::msg)?),
                idempotency_key: Some(key.clone().into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                environment_id: Some(component.environment_id.into()),
                component_owner_account_id: Some(component.account_id.into()),
                mode: AgentInvocationMode::Await as i32,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
                request_key: Some(
                    golem_common::model::InvocationRequestKey::external(
                        executor.auth_ctx().idempotency_scope(),
                        Some(&key),
                    )
                    .into(),
                ),
                ..Default::default()
            })),
        };
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&request)
            .map_err(anyhow::Error::msg)?;
        let (requests, receiver) = tokio::sync::mpsc::channel(1);
        requests.send(request).await?;
        let mut responses = executor
            .client
            .clone()
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let mut items = Vec::new();
        let mut result_seen = false;
        let mut finishes = 0;
        let mut output_terminals = 0;
        while let Some(response) = responses.message().await? {
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
            match response.response {
                Some(invocation_response::Response::Accepted(_)) => {}
                Some(invocation_response::Response::Result(_)) => result_seen = true,
                Some(invocation_response::Response::OutputItem(item)) => {
                    assert!(result_seen, "producer ran before its stream was returned");
                    assert_eq!(item.producer_sequence, items.len() as u64);
                    let Some(golem_schema::proto::golem::schema::schema_value::Value::StringValue(
                        value,
                    )) = item.value.and_then(|value| value.value)
                    else {
                        anyhow::bail!("expected string output");
                    };
                    items.push(value);
                    if items.len() == 1 {
                        assert_eq!(items[0], "before-terminal");
                        executor.complete_promise(&gate, Vec::new()).await?;
                    }
                }
                Some(invocation_response::Response::StreamEnd(end)) => {
                    output_terminals += 1;
                    match StreamTerminal::try_from(end.outcome.expect("output terminal"))
                        .map_err(anyhow::Error::msg)?
                    {
                        StreamTerminal::Finished => assert_eq!(mode, "complete"),
                        StreamTerminal::Failed(error) => {
                            assert_ne!(mode, "complete", "successful producer failed: {error:?}");
                        }
                        StreamTerminal::ConsumerCancelled { .. } => {
                            anyhow::bail!("producer output was cancelled by its consumer");
                        }
                    }
                }
                Some(invocation_response::Response::Finished(finished)) => {
                    finishes += 1;
                    match finished.outcome {
                        Some(invocation_session_completion::Outcome::Success(_)) => {
                            assert_eq!(mode, "complete")
                        }
                        Some(invocation_session_completion::Outcome::Failure(_)) => {
                            assert_ne!(mode, "complete")
                        }
                        None => anyhow::bail!("missing invocation outcome"),
                    }
                }
                other => anyhow::bail!("unexpected producer response: {other:?}"),
            }
        }
        assert!(state.is_complete());
        assert_eq!(finishes, 1);
        assert_eq!(output_terminals, 1);
        let oplog = executor.get_oplog(&worker, checkpoint.next()).await?;
        let recorded = recorded_string_items(&oplog)?;
        if mode == "complete" {
            assert_eq!(items, ["before-terminal", "completed"]);
            assert_eq!(recorded, items);
        } else {
            assert_eq!(recorded, ["before-terminal"]);
            assert_eq!(items, recorded);
            executor
                .wait_for_status(&worker, AgentStatus::Failed, Duration::from_secs(10))
                .await?;
            let errors: Vec<_> = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Error(error) => Some(error.error.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(errors.len(), 1, "{mode}: {errors:?}");
            if mode == "deadline" {
                assert!(
                    errors[0].contains("maximum invocation duration of 2s"),
                    "{errors:?}"
                );
            } else {
                assert!(errors[0].contains("unreachable"), "{errors:?}");
            }
            assert!(
                !oplog.iter().any(|entry| matches!(
                    entry.entry,
                    PublicOplogEntry::AgentInvocationFinished(_)
                ))
            );
        }
    }
    Ok(())
}

/// The caller starts a short tool call, blocks once on a timer and then only yields while it polls
/// the result. A yield is not a blocked wait, so the result must not reach the guest while it
/// yields, and a rebuild must replay the call with the result at the same guest boundary.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn yielding_guest_receives_tool_result_only_at_a_blocked_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
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
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!("ToolStreamingCaller", "yielding-tool-result");
    let worker_id = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;

    let observed_while_yielding: bool = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "yielding_tool_result_observation",
            data_value!(100_000u32),
        )
        .await?
        .into_typed()?;
    assert!(
        !observed_while_yielding,
        "the tool result reached a guest that only yielded"
    );
    let recorded = executor.stored_oplog(&worker_id).await;

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed: String = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the rebuild did not replay the tool result"))??
    .into_typed()?;
    assert_eq!(replayed, "replayed");
    let rebuilt = executor.stored_oplog(&worker_id).await;
    assert_eq!(
        &rebuilt[..recorded.len()],
        &recorded[..],
        "the rebuild changed the recorded history"
    );
    Ok(())
}
