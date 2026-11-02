// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::oplog::attachment::{ToolAttachmentReadObservation, ToolAttachmentSlot};
use test_r::test;

#[test]
#[timeout("3m")]
async fn component_attachment_payloads_follow_consumption_across_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (label, capacity) in [
        ("asymmetric", 17),
        ("completion", 64),
        ("large", 4096),
        ("unread", 32),
    ] {
        let context = TestContext::new(last_unique_id);
        let environment = Arc::new(TestEnvironmentStateService::default());
        let overrides = TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            configure: Some(Arc::new(move |config| {
                config.limits.max_tool_attachment_bytes = capacity;
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
        let agent = agent_id!("ToolStreamingCaller", label);
        let worker = executor
            .start_agent(&caller_component.id, agent.clone())
            .await?;
        let input: Vec<u8> = (0..127).map(|i| (i * 73 + 19) as u8).collect();
        let (method, arguments, expected) = match label {
            "unread" => (
                "retain_tool_output",
                data_value!(false, true, false),
                b"marker:".to_vec(),
            ),
            "completion" => (
                "collect_capable",
                data_value!("stdout-exact:/cost.bin", vec![0x93_u8; 64]),
                vec![b'x'; 64],
            ),
            "large" => (
                "collect",
                data_value!("large", Vec::<u8>::new(), 1_u32),
                (0..512)
                    .flat_map(|i| std::iter::repeat_n((i % 251) as u8, 4096))
                    .collect(),
            ),
            _ => ("collect", data_value!("echo", input.clone(), 11_u32), input),
        };
        let result = executor
            .invoke_and_await_agent(&caller_component, &agent, method, arguments)
            .await?;
        if label == "unread" {
            assert!(result.into_typed::<Vec<u8>>()?.is_empty());
        } else {
            let result: StreamEvidence = result.into_typed()?;
            assert_eq!(result.output, expected);
            assert_eq!(result.completion, "ok");
        }
        let history = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let mut starts = BTreeSet::new();
        let mut output_payload = Vec::new();
        let mut reads = 0;
        for entry in &history {
            match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name.ends_with("-writer::write") =>
                {
                    starts.insert(entry.oplog_index);
                }
                PublicOplogEntry::ToolAttachmentRead(read) => {
                    reads += 1;
                    let observation =
                        ToolAttachmentReadObservation::from_value(read.observation.value())
                            .unwrap();
                    if let Some(bytes) = &read.payload {
                        assert!(observation.reobserves.is_none());
                        if matches!(
                            observation.attachment.slot,
                            ToolAttachmentSlot::Stdout | ToolAttachmentSlot::Output
                        ) {
                            output_payload.extend_from_slice(bytes);
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(
            output_payload, expected,
            "original output observations own exactly consumed bytes"
        );
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        assert_eq!(executor.instance_load_count(&worker), 0);
        let _: String = executor
            .invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!())
            .await?
            .into_typed()?;
        assert!(executor.instance_load_count(&worker) > 0);
        let replayed = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        assert_eq!(
            replayed
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::ToolAttachmentRead(_)))
                .count(),
            reads
        );
        assert_eq!(replayed.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name.ends_with("-writer::write"))).count(), starts.len());
        if label == "unread" {
            let unread: Vec<u8> = executor
                .invoke_and_await_agent(
                    &caller_component,
                    &agent,
                    "consume_retained_tool_output",
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_eq!(
                unread,
                [vec![0, 255, 19], b"retained-tail".to_vec()].concat()
            );
        }
        executor.delete_worker(&worker).await?;
    }
    Ok(())
}
