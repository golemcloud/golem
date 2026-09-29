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

use super::*;
use golem_common::model::oplog::{OplogEntry, OplogErrorKind, PublicOplogEntryWithIndex};
use golem_worker_executor::services::HasOplogService;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn nested_leaf_trap_notifies_external_caller_while_primary_waits_on_silent_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let silent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let silent_port = silent_listener.local_addr()?.port();
    let (checkpoint_port, gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let _checkpoint_server = tokio_util::task::AbortOnDropHandle::new(checkpoint_server);
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_trapped_leaf_observer_release",
        )
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-early-return-failure")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let early = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-early-return")
        .unwrap();
    let agent_type = AgentTypeName("TrappedLeafObserver".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(early.name.as_str(), empty_middleware_parameters(early))],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("TrappedLeafObserver", "silent-primary");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PRIMARY_SILENT_TCP_PORT".to_string(),
                    silent_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    worker.await_ready_to_process_commands().await?;

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "nested_trap_while_primary_receives",
        data_value!(),
    );
    tokio::pin!(invocation);
    let silent_peer = tokio::select! {
        result = invocation.as_mut() => anyhow::bail!("invocation ended before the primary connected: {result:?}"),
        accepted = tokio::time::timeout(std::time::Duration::from_secs(30), silent_listener.accept()) => accepted??.0,
    };
    let child_gate = tokio::select! {
        result = invocation.as_mut() => anyhow::bail!("invocation ended before the nested child gate: {result:?}"),
        arrival = next_crash_checkpoint(&mut arrivals, "middleware-early-child") => arrival?,
    };
    let ready = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            // Flush accepted Starts so the held schedule can be inspected in storage.
            executor.commit_oplog(&worker_id).await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let parent = oplog.iter().find_map(|entry| {
                matches!(&entry.attribution, PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == PublicAgentEntityKind::ToolMiddleware
                        && entity.invocation.entity.name == "streaming-early-return"
                        && entity.ancestors.is_empty())
                .then_some(entry)
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(p)
                    if p.function_name == "golem::entity::invoke"))
                .map(|entry| entry.oplog_index)
            });
            let primary_waits = oplog.iter().filter_map(|entry| {
                matches!(&entry.attribution, PublicOplogEntryAttribution::Agent(_))
                    .then_some(entry)
                    .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(p)
                        if p.function_name == "sockets::types::tcp-socket::receive"
                            || p.function_name == "sockets::types::tcp-socket::receive-chunk"))
                    .map(|entry| entry.oplog_index)
            }).collect::<Vec<_>>();
            if let Some(parent) = parent {
                let child = oplog.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(p) if p.function_name == "golem::entity::invoke"
                        && p.parent_start_index == Some(parent) => Some(entry.oplog_index),
                    _ => None,
                });
                if let Some(child) = child
                    && has_end(&oplog, parent)
                    && primary_waits.len() == 2
                {
                    for start in primary_waits.iter().copied().chain([child]) {
                        assert_incomplete(&oplog, start);
                    }
                    let operations = worker.owner_execution().tool_operation_metadata();
                    assert_eq!(operations.operations.len(), 2);
                    assert_eq!(operations.owner_failure, None);
                    let tip = worker.oplog_service().get_last_index(&owned, AgentMode::Durable).await;
                    let committed = worker.oplog_service().read_exact(
                        &owned, AgentMode::Durable, OplogIndex::INITIAL, tip.as_u64(),
                    ).await;
                    for start in [parent, child].into_iter().chain(primary_waits.iter().copied()) {
                        assert!(matches!(committed.get(&start), Some(OplogEntry::Start { .. })));
                    }
                    eprintln!("SILENT_PRIMARY_READY parent={parent:?} child={child:?} primary_waits={primary_waits:?} committed_tip={tip:?} operations={operations:#?}");
                    break Ok::<_, anyhow::Error>((parent, child, primary_waits));
                }
            }
            tokio::select! {
                result = invocation.as_mut() => anyhow::bail!("invocation ended before accepted primary wait and nested child: {result:?}"),
                () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {},
            }
        }
    }).await.map_err(|_| anyhow::anyhow!("primary silent receive / parent End / child Start schedule was not reached"))??;
    let (parent, child, primary_waits) = ready;
    child_gate
        .release
        .send(())
        .expect("release independently trapping child");
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(20), invocation.as_mut()).await;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let operations = worker.owner_execution().tool_operation_metadata();
    let active = executor.active_entity_metadata(&owned).await;
    eprintln!(
        "SILENT_PRIMARY_OUTCOME caller={result:#?} status={metadata:#?} operations={operations:#?} active={active:#?} silent_peer_still_open={:?}",
        silent_peer.peer_addr()
    );
    let tip = worker
        .oplog_service()
        .get_last_index(&owned, AgentMode::Durable)
        .await;
    let committed = worker
        .oplog_service()
        .read_exact(
            &owned,
            AgentMode::Durable,
            OplogIndex::INITIAL,
            tip.as_u64(),
        )
        .await;
    let failures: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
                PublicOplogEntry::Error(p) if p.kind == OplogErrorKind::Invocation)
        })
        .collect();
    eprintln!("SILENT_PRIMARY_COMMITTED tip={tip:?} invocation_errors={failures:#?}");
    assert_eq!(
        operations.owner_failure,
        Some(ToolOwnerFailureMetadata::Trap),
        "the nested child must select Trap, not merely leave the primary waiting"
    );
    assert!(has_end(&oplog, parent));
    assert_incomplete(&oplog, child);
    for start in primary_waits {
        assert_incomplete(&oplog, start);
    }
    let result = result.map_err(|_| anyhow::anyhow!("external caller hung for 20s after nested leaf release while primary silent TCP peer remained open; selected={:?}; status={:?}", operations.owner_failure, metadata.status))?;
    let error = result.expect_err("nested child trap must fail the external invocation");
    assert!(
        error
            .to_string()
            .contains("nested middleware child trap after parent return"),
        "must report the original selected trap: {error:#?}"
    );
    assert_eq!(metadata.status, AgentStatus::Failed);
    assert_eq!(failures.len(), 1);
    assert!(matches!(
        committed.get(&failures[0].oplog_index),
        Some(OplogEntry::Error {
            kind: OplogErrorKind::Invocation,
            ..
        })
    ));
    assert!(!oplog.iter().any(|entry| entry.oplog_index > parent
        && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))));
    drop(silent_peer);
    Ok(())
}

fn has_end(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> bool {
    oplog
        .iter()
        .any(|entry| matches!(&entry.entry, PublicOplogEntry::End(p) if p.start_index == start))
}

fn assert_incomplete(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) {
    assert!(
        !has_end(oplog, start),
        "unfinished Start {start:?} must not have End"
    );
    assert!(
        !oplog.iter().any(
            |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(p) if p.start_index == start)
        ),
        "unfinished Start {start:?} must not have Cancelled"
    );
}
