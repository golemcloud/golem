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
use anyhow::{Context as _, ensure};
use golem_common::model::card::{
    InvocationWalletPin, PublicInvocationWalletPin, WalletVersionToken,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{AttributeValue, SpanId, TraceId};
use golem_common::model::oplog::payload::types::encode_span_data;
use golem_common::model::oplog::{
    AttributeMap, HostRequest, HostResponse, OplogEntry, OplogPayload, PublicAttribute,
    PublicAttributeValue, PublicEntityCallMode, PublicEntityInvocationOperation,
    PublicSpanAttributes, PublicSpanData, PublicSpanKind, PublicSpanStarted, SpanAttributes,
    SpanData, SpanKind, SpanStarted, StringAttributeValue,
};
use golem_common::model::{AgentFingerprint, AgentId, AgentInvocationPayload};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use golem_worker_executor::services::HasOplog;
use golem_worker_executor::services::blob_store::{
    BlobStoreError, BlobStoreService, DefaultBlobStoreService,
};
use golem_worker_executor::services::oplog::{Oplog, OplogOps};
use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::start_with_resource_limits_and_overrides;
use std::path::Path;
use test_r::test;

use crate::blob_mutation_storage::MutationStorage;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);

struct SharedBlobLimits(Arc<AtomicResourceEntry>);

#[async_trait::async_trait]
impl ResourceLimits for SharedBlobLimits {
    async fn initialize_account(
        &self,
        _account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        Ok(self.0.clone())
    }
}

struct ReleaseMutation(Arc<MutationStorage>);

impl Drop for ReleaseMutation {
    fn drop(&mut self) {
        self.0.release.add_permits(1);
    }
}

#[test]
#[timeout("2m")]
async fn durable_entity_blob_write_completion_control(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    write_and_reconstruct(last_unique_id, deps, provider, false).await
}

#[test]
#[timeout("2m")]
async fn durable_entity_blob_write_survives_owner_fence_and_repairs_same_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    write_and_reconstruct(last_unique_id, deps, provider, true).await
}

async fn invoke_blob_tool(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    key: &IdempotencyKey,
    input: &TypedSchemaValue,
    principal: Principal,
) -> Result<AgentInvocationResult, WorkerExecutorError> {
    let active = executor
        .production_active_agent(owner)
        .await
        .ok_or_else(|| WorkerExecutorError::runtime("expected the existing durable owner"))?;
    active
        .primary()
        .invoke_external_tool(
            fingerprint,
            key.clone(),
            ToolName::try_from("clock-race").unwrap(),
            vec!["write-blob".into()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await
        .map(|output| output.result)
}

fn assert_output(result: AgentInvocationResult) -> anyhow::Result<()> {
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = result else {
        anyhow::bail!("expected successful real tool result, got {result:?}");
    };
    ensure!(
        result.result.map(|value| value.into_parts().1)
            == Some(SchemaValue::String(
                "blob:container/destination:1357".into()
            ))
    );
    Ok(())
}

fn blob_input(definition: &golem_common::schema::tool::Tool) -> anyhow::Result<TypedSchemaValue> {
    let command = definition
        .command_index_by_path(&["write-blob".into()])
        .context("provider fixture must be rebuilt with clock-race/write-blob")?;
    let schema = definition.canonical_input_record_schema(command)?;
    let SchemaType::Record { fields, .. } = &schema.root else {
        anyhow::bail!("write-blob canonical input must be a record");
    };
    let mut values = BTreeMap::from([
        ("container", SchemaValue::String("container".into())),
        ("object", SchemaValue::String("destination".into())),
        ("contents", SchemaValue::String("1357".into())),
    ]);
    let ordered = fields
        .iter()
        .map(|field| {
            values
                .remove(field.name.as_str())
                .context("unexpected write-blob field")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ensure!(values.is_empty());
    Ok(TypedSchemaValue::new(
        schema,
        SchemaValue::Record { fields: ordered },
    ))
}

async fn write_and_reconstruct(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    fence: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_id = context.default_environment_id;
    let storage = Arc::new(MutationStorage::default());
    let _release = ReleaseMutation(storage.clone());
    let service = Arc::new(DefaultBlobStoreService::new(storage.clone()));
    let limits = Arc::new(AtomicResourceEntry::new_with_blob_storage_limit(10));
    service
        .create_container(environment_id, "container".into())
        .await
        .map_err(anyhow::Error::msg)?;
    for (object, bytes, delta) in [("source", &b"seed"[..], 4), ("destination", &b"z"[..], 1)] {
        let mutation = service
            .write_data(limits.clone(), environment_id, "container", object, bytes)
            .await
            .map_err(anyhow::Error::msg)?;
        ensure!(mutation.bytes_delta == delta);
    }
    let environment = Arc::new(TestEnvironmentStateService::default());
    let resource_limits = Arc::new(SharedBlobLimits(limits.clone()));
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        wrap_blob_store_service: Some(Arc::new({
            let service = service.clone();
            move |_| service.clone()
        })),
        ..Default::default()
    };
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        resource_limits.clone(),
        overrides.clone(),
    )
    .await?;
    let component = executor
        .component_dep(&environment_id, provider)
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
                .is_some_and(|root| root.name == "clock-race")
        })
        .context("provider must export real clock-race metadata")?;
    let input = blob_input(definition)?;
    environment.set_tool_deployment(
        environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            context.account_id,
            component.id,
            component.revision,
            "golem-it:tool-streaming-rust-provider",
            "NapFetchOwner",
            metadata.tools,
        )),
    );
    let name = agent_id!("NapFetchOwner", format!("entity-blob-{fence}"));
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let fingerprint = executor.get_worker_metadata(&id).await?.fingerprint;
    let owner = OwnedAgentId::new(environment_id, &id);
    let active = executor
        .production_active_agent(&owner)
        .await
        .context("existing durable owner")?;
    let worker = active.primary();
    let resources = active.resources();
    let key = IdempotencyKey::fresh();
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    storage.armed.store(true, Ordering::SeqCst);
    let mut invocation = Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let executor = executor.clone();
        let owner = owner.clone();
        let key = key.clone();
        let input = input.clone();
        let principal = principal.clone();
        async move { invoke_blob_tool(&executor, &owner, fingerprint, &key, &input, principal).await }
    })));
    storage.pending.notified().await;
    ensure!(!invocation.as_ref().unwrap().is_finished());
    ensure!(storage.entered.load(Ordering::SeqCst) == 3);
    ensure!(storage.effects.load(Ordering::SeqCst) == 2);
    ensure!(storage.completed.load(Ordering::SeqCst) == 0);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 0);
    ensure!(!limits.try_record_blob_storage_delta(3));
    ensure!(service.mutation_failure(environment_id).is_none());
    ensure!(worker.concurrent_agent_permit_is_held().await);
    executor.commit_oplog(&id).await?;
    let before = capture_snapshot(&executor, &id, worker.oplog()).await?;
    let window = read_window(&before.public, &key, &input, 0)?;
    assert_terminals(&before.public, window, false)?;
    let slot = active
        .entity_slots()
        .into_iter()
        .find(|slot| {
            slot.active_invocations()
                .iter()
                .any(|invocation| invocation.invocation_id.start_index() == window.root)
        })
        .context("actual component runner slot")?;
    let registered = slot.active_invocations();
    ensure!(registered.len() == 1 && registered[0].store_attached);
    ensure!(registered[0].linear_memory_bytes > 0);
    ensure!(active.execution().lane().metadata().active_invocation_count == 2);
    ensure!(resources.live_usage_flusher_count_for_test() == 2);
    let mut competing = Box::pin(service.write_data(
        limits.clone(),
        environment_id,
        "container",
        "competing",
        b"123",
    ));
    ensure!(futures::poll!(&mut competing).is_pending());

    let stopped = if fence {
        executor.interrupt(&id).await?;
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await?;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.unload_succeeded_for_test());
        ensure!(invocation.take().unwrap().await?.is_err());
        ensure!(slot.active_invocation_count() == 0 && slot.charged_linear_memory_bytes() == 0);
        ensure!(!slot.is_accepting());
        ensure!(active.execution().lane().metadata().active_invocation_count == 0);
        ensure!(resources.live_usage_flusher_count_for_test() == 0);
        ensure!(
            storage.dropped.load(Ordering::SeqCst) == 0,
            "whole ComponentEntityRunner cancellation dropped the reserved provider"
        );
        ensure!(storage.completed.load(Ordering::SeqCst) == 0);
        ensure!(!limits.try_record_blob_storage_delta(3));
        ensure!(service.mutation_failure(environment_id).is_none());
        ensure!(
            futures::poll!(&mut competing).is_pending(),
            "caller loss released the environment mutex"
        );
        let stopped = capture_snapshot(&executor, &id, worker.oplog()).await?;
        assert_prefix(&before, &stopped)?;
        ensure!(read_window(&stopped.public, &key, &input, 0)? == window);
        assert_terminals(&stopped.public, window, false)?;
        assert_bytes(&storage, environment_id, b"z").await?;
        Some(stopped)
    } else {
        None
    };

    storage.release.add_permits(1);
    ensure!(matches!(
        competing.await,
        Err(BlobStoreError::LimitExceeded(_))
    ));
    ensure!(storage.completed.load(Ordering::SeqCst) == 1);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 0);
    ensure!(storage.entered.load(Ordering::SeqCst) == 3);
    ensure!(storage.effects.load(Ordering::SeqCst) == 3);
    assert_bytes(&storage, environment_id, b"1357").await?;
    assert_exact_stock_eight(&service, limits.clone(), environment_id).await?;
    if let Some(stopped) = stopped {
        let settled = capture_snapshot(&executor, &id, worker.oplog()).await?;
        assert_snapshots_equal(&stopped, &settled)
            .context("service settlement wrote or changed an owner-oplog record")?;
        assert_terminals(&settled.public, window, false)?;
        executor.resume(&id, false).await?;
        assert_output(
            invoke_blob_tool(
                &executor,
                &owner,
                fingerprint,
                &key,
                &input,
                principal.clone(),
            )
            .await?,
        )?;
    } else {
        assert_output(invocation.take().unwrap().await??)?;
        ensure!(slot.active_invocation_count() == 0);
    }
    let completed_owner = executor
        .production_active_agent(&owner)
        .await
        .context("completed durable owner")?;
    let completed = capture_snapshot(&executor, &id, completed_owner.primary().oplog()).await?;
    assert_prefix(&before, &completed)?;
    ensure!(read_window(&completed.public, &key, &input, 1)? == window);
    assert_terminals(&completed.public, window, true)?;
    let expected_puts = if fence { 5 } else { 4 };
    ensure!(storage.entered.load(Ordering::SeqCst) == expected_puts);
    ensure!(
        storage.effects.load(Ordering::SeqCst) == expected_puts,
        "incomplete repair may repeat the same-key put, completed replay may not"
    );
    ensure!(storage.completed.load(Ordering::SeqCst) == 1);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 0);
    assert_exact_stock_eight(&service, limits.clone(), environment_id).await?;
    let expected_puts = expected_puts + 1;
    ensure!(storage.entered.load(Ordering::SeqCst) == expected_puts);
    ensure!(storage.effects.load(Ordering::SeqCst) == expected_puts);
    assert_bytes(&storage, environment_id, b"1357").await?;
    assert_output(
        invoke_blob_tool(&executor, &owner, fingerprint, &key, &input, principal).await?,
    )?;
    ensure!(storage.effects.load(Ordering::SeqCst) == expected_puts);

    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor =
        start_with_resource_limits_and_overrides(deps, &context, resource_limits, overrides)
            .await?;
    ensure!(executor.get_worker_metadata(&id).await?.fingerprint == fingerprint);
    let follow_up = executor
        .invoke_and_await_agent(&component, &name, "follow_up", data_value!())
        .await?
        .into_typed::<String>()?;
    ensure!(follow_up == "settled");
    let reconstructed_owner = executor
        .production_active_agent(&owner)
        .await
        .context("reconstructed durable owner")?;
    let reconstructed =
        capture_snapshot(&executor, &id, reconstructed_owner.primary().oplog()).await?;
    assert_prefix(&completed, &reconstructed)?;
    ensure!(read_window(&reconstructed.public, &key, &input, 1)? == window);
    assert_terminals(&reconstructed.public, window, true)?;
    ensure!(storage.entered.load(Ordering::SeqCst) == expected_puts);
    ensure!(storage.effects.load(Ordering::SeqCst) == expected_puts);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 0);
    assert_exact_stock_eight(&service, limits.clone(), environment_id).await?;
    ensure!(storage.entered.load(Ordering::SeqCst) == expected_puts + 1);
    ensure!(storage.effects.load(Ordering::SeqCst) == expected_puts + 1);
    assert_bytes(&storage, environment_id, b"1357").await?;
    let primary = reconstructed_owner.primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while primary.eviction_class().await != Some(EvictionClass::LoadedIdle) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("completed owner did not become idle for final eviction")?;
    ensure!(primary.stop_if_evictable(EvictionClass::LoadedIdle).await);
    primary.await_ready_to_process_commands().await?;
    primary.retained_cleanup_for_test().await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    ensure!(!reconstructed_owner.primary().is_loaded().await);
    ensure!(
        !reconstructed_owner
            .primary()
            .concurrent_agent_permit_is_held()
            .await
    );
    ensure!(
        reconstructed_owner
            .entity_slots()
            .iter()
            .all(|slot| slot.active_invocation_count() == 0)
    );
    ensure!(
        reconstructed_owner
            .execution()
            .lane()
            .metadata()
            .active_invocation_count
            == 0
    );
    ensure!(
        reconstructed_owner
            .resources()
            .live_usage_flusher_count_for_test()
            == 0
    );
    Ok(())
}

async fn assert_bytes(
    storage: &MutationStorage,
    environment_id: EnvironmentId,
    destination: &[u8],
) -> anyhow::Result<()> {
    for (object, expected) in [("source", &b"seed"[..]), ("destination", destination)] {
        ensure!(
            storage
                .get_raw(
                    "test",
                    "read",
                    BlobStorageNamespace::CustomStorage { environment_id },
                    &Path::new("container").join(object),
                )
                .await?
                == Some(expected.to_vec())
        );
    }
    Ok(())
}

async fn assert_exact_stock_eight(
    service: &DefaultBlobStoreService,
    limits: Arc<AtomicResourceEntry>,
    environment_id: EnvironmentId,
) -> anyhow::Result<()> {
    ensure!(
        service
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "quota-probe",
                b"24"
            )
            .await
            .map_err(anyhow::Error::msg)?
            .bytes_delta
            == 2
    );
    ensure!(matches!(
        service
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "over-cap",
                b"6"
            )
            .await,
        Err(BlobStoreError::LimitExceeded(_))
    ));
    ensure!(
        service
            .delete_object(
                limits,
                environment_id,
                "container".into(),
                "quota-probe".into()
            )
            .await
            .map_err(anyhow::Error::msg)?
            .bytes_delta
            == -2
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    invocation: OplogIndex,
    root: OplogIndex,
    blob: OplogIndex,
}

fn read_window(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    input: &TypedSchemaValue,
    finished: usize,
) -> anyhow::Result<Window> {
    let matches = |invocation: &PublicAgentInvocation| {
        matches!(invocation,
        PublicAgentInvocation::ExternalTool(call) if &call.idempotency_key == key
            && call.tool_name == "clock-race" && call.command_path == ["write-blob"] && &call.input == input)
    };
    let mut accepted = 0;
    let mut invocations = Vec::new();
    let mut current = false;
    let mut completions = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::PendingAgentInvocation(call) if matches(&call.invocation) => {
                accepted += 1
            }
            PublicOplogEntry::AgentInvocationStarted(call) => {
                current = matches(&call.invocation);
                if current {
                    invocations.push(entry.oplog_index);
                }
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => completions += 1,
            _ => {}
        }
    }
    ensure!(
        accepted == 1 && invocations.len() == 1 && completions == finished,
        "key {key}: {accepted} accepted, {invocations:?} Started, {completions} Finished"
    );
    let invocation = invocations[0];
    let starts = |name: &str, parent| {
        entries
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(call)
                    if call.function_name.ends_with(name)
                        && call.parent_start_index == Some(parent) =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let roots = starts("golem::entity::invoke", invocation);
    ensure!(
        roots.len() == 1,
        "expected one actual durable tool root: {roots:?}"
    );
    let root = roots[0];
    let blobs = starts("::write_data", root);
    ensure!(
        blobs.len() == 1,
        "repair must retain the original blob Start: {blobs:?}"
    );
    let blob = blobs[0];
    for index in [root, blob] {
        let entry = entries
            .iter()
            .find(|entry| entry.oplog_index == index)
            .unwrap();
        let PublicOplogEntryAttribution::Entity(attribution) = &entry.attribution else {
            anyhow::bail!("durable root must provide real entity attribution: {entry:?}");
        };
        ensure!(attribution.invocation.start_index == root);
        ensure!(attribution.invocation.entity.kind == PublicAgentEntityKind::Tool);
        ensure!(
            attribution.invocation.entity.name == "clock-race" && attribution.ancestors.is_empty()
        );
        ensure!(attribution.invocation.call_mode == PublicEntityCallMode::Synchronous);
        ensure!(matches!(&attribution.invocation.operation,
            Some(PublicEntityInvocationOperation::Tool(operation)) if operation.command_path == ["write-blob"]));
    }
    let reads = starts("::get_container", root);
    ensure!(reads.len() == 1 && reads[0] < blob);
    ensure!(entries.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::End(end) if end.start_index == reads[0])
        && entry.oplog_index < blob));
    Ok(Window {
        invocation,
        root,
        blob,
    })
}

fn assert_terminals(
    entries: &[PublicOplogEntryWithIndex],
    window: Window,
    completed: bool,
) -> anyhow::Result<()> {
    for start in [window.root, window.blob] {
        let ends = entries
            .iter()
            .filter_map(|entry| {
                matches!(&entry.entry,
            PublicOplogEntry::End(end) if end.start_index == start)
                .then_some(entry.oplog_index)
            })
            .collect::<Vec<_>>();
        ensure!(
            ends.len() == usize::from(completed),
            "wrong terminal count for {start}: {ends:?}"
        );
        ensure!(!entries.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start)));
        let delivered = entries
            .iter()
            .filter_map(|entry| {
                matches!(&entry.entry,
            PublicOplogEntry::CompletionDelivered(marker) if marker.start_index == start)
                .then_some(entry.oplog_index)
            })
            .collect::<Vec<_>>();
        ensure!(
            delivered.is_empty(),
            "host-internal tool root and serialized blob write have no delivery marker for {start}: {delivered:?}"
        );
        ensure!(!entries.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::CompletionDiscarded(marker) if marker.start_index == start)));
    }
    if completed {
        let finished = entries
            .iter()
            .find(|entry| {
                entry.oplog_index > window.blob
                    && matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            })
            .context("original invocation Finished")?
            .oplog_index;
        ensure!(entries.iter().filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::End(end) if end.start_index == window.root || end.start_index == window.blob))
            .all(|entry| entry.oplog_index < finished));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct OplogSnapshot {
    raw: Vec<OplogEntry>,
    public: Vec<PublicOplogEntryWithIndex>,
    payloads: Vec<(OplogIndex, RecordedPayload)>,
}

#[derive(Clone, Debug, PartialEq)]
enum RecordedPayload {
    Invocation(Box<AgentInvocationPayload>),
    Result(Box<AgentInvocationResult>),
    Request(Box<HostRequest>),
    Response(Box<HostResponse>),
}

async fn capture_snapshot(
    executor: &TestWorkerExecutor,
    id: &AgentId,
    oplog: Arc<dyn Oplog>,
) -> anyhow::Result<OplogSnapshot> {
    let public = executor.get_oplog(id, OplogIndex::INITIAL).await?;
    // This reads the stored service, not the live actor's buffered objects. Keep the full
    // raw snapshot even if an append advanced its tip after the public query.
    let raw = executor.stored_oplog(id).await;
    let mut payloads = Vec::new();
    for (offset, entry) in raw.iter().enumerate() {
        let payload = match entry {
            OplogEntry::AgentInvocationStarted { payload, .. }
            | OplogEntry::PendingAgentInvocation { payload, .. } => {
                Some(RecordedPayload::Invocation(Box::new(
                    oplog
                        .download_payload(payload.clone())
                        .await
                        .map_err(anyhow::Error::msg)?,
                )))
            }
            OplogEntry::AgentInvocationFinished { result, .. } => {
                Some(RecordedPayload::Result(Box::new(
                    oplog
                        .download_payload(result.clone())
                        .await
                        .map_err(anyhow::Error::msg)?,
                )))
            }
            OplogEntry::Start {
                request: Some(request),
                ..
            } => Some(RecordedPayload::Request(Box::new(
                oplog
                    .download_payload(request.clone())
                    .await
                    .map_err(anyhow::Error::msg)?,
            ))),
            OplogEntry::End {
                response: Some(response),
                ..
            }
            | OplogEntry::Cancelled {
                partial: Some(response),
                ..
            } => Some(RecordedPayload::Response(Box::new(
                oplog
                    .download_payload(response.clone())
                    .await
                    .map_err(anyhow::Error::msg)?,
            ))),
            _ => None,
        };
        if let Some(payload) = payload {
            payloads.push((record_index(offset), payload));
        }
    }
    let snapshot = OplogSnapshot {
        raw,
        public,
        payloads,
    };
    validate_public_projection(&snapshot)?;
    Ok(snapshot)
}

fn record_index(offset: usize) -> OplogIndex {
    OplogIndex::from_u64(OplogIndex::INITIAL.as_u64() + offset as u64)
}

fn assert_prefix(prefix: &OplogSnapshot, entries: &OplogSnapshot) -> anyhow::Result<()> {
    validate_public_projection(prefix)?;
    validate_public_projection(entries)?;
    ensure!(
        entries.raw.len() >= prefix.raw.len(),
        "stored prefix lost records"
    );
    for (offset, (expected, actual)) in prefix.raw.iter().zip(&entries.raw).enumerate() {
        ensure!(
            expected == actual,
            "stored oplog prefix changed at {}: expected={expected:#?}, actual={actual:#?}",
            record_index(offset)
        );
    }
    ensure!(
        entries.payloads.len() >= prefix.payloads.len(),
        "stored prefix lost payloads"
    );
    ensure!(
        prefix.payloads.as_slice() == &entries.payloads[..prefix.payloads.len()],
        "hydrated invocation or host-call payload prefix changed"
    );
    ensure!(
        entries.public.len() >= prefix.public.len(),
        "public prefix lost records"
    );
    for (expected, actual) in prefix.public.iter().zip(&entries.public) {
        ensure!(expected.oplog_index == actual.oplog_index);
        ensure!(
            expected.attribution == actual.attribution,
            "public attribution changed at {}",
            expected.oplog_index
        );
        assert_public_facts_equal(&expected.entry, &actual.entry)
            .with_context(|| format!("public facts changed at {}", expected.oplog_index))?;
    }
    Ok(())
}

fn assert_snapshots_equal(expected: &OplogSnapshot, actual: &OplogSnapshot) -> anyhow::Result<()> {
    ensure!(
        expected.raw.len() == actual.raw.len(),
        "stored oplog length changed"
    );
    ensure!(
        expected.public.len() == actual.public.len(),
        "public oplog length changed"
    );
    ensure!(expected.payloads.len() == actual.payloads.len());
    assert_prefix(expected, actual)
}

fn recorded_attributes(attributes: &AttributeMap) -> Vec<PublicAttribute> {
    attributes
        .0
        .iter()
        .map(|(key, value)| PublicAttribute {
            key: key.clone(),
            value: value.clone().into(),
        })
        .collect()
}

fn assert_attributes(
    recorded: &[PublicAttribute],
    public: &[PublicAttribute],
) -> anyhow::Result<()> {
    ensure!(
        recorded.len() == public.len(),
        "attribute cardinality changed"
    );
    let mut recorded_keys = BTreeSet::new();
    for attribute in recorded {
        ensure!(
            recorded_keys.insert(&attribute.key),
            "duplicate recorded attribute key"
        );
    }
    let mut public_keys = BTreeSet::new();
    for attribute in public {
        ensure!(
            public_keys.insert(&attribute.key),
            "duplicate public attribute key"
        );
        let expected = recorded
            .iter()
            .find(|expected| expected.key == attribute.key)
            .context("unrecorded public attribute key")?;
        ensure!(
            expected.value == attribute.value,
            "typed attribute value changed for {}",
            attribute.key
        );
    }
    Ok(())
}

fn assert_context_projection(
    recorded: &[SpanData],
    public: &[Vec<PublicSpanData>],
) -> anyhow::Result<()> {
    // Use the actual producer for the stored span stack and linked-context layout. Only
    // its map-backed attribute lists have unordered semantics; stack/node order does not.
    let expected = encode_span_data(recorded);
    ensure!(
        expected.len() == public.len(),
        "span context stack count changed"
    );
    for (expected, actual) in expected.iter().zip(public) {
        ensure!(
            expected.len() == actual.len(),
            "span context node count changed"
        );
        for (expected, actual) in expected.iter().zip(actual) {
            match (expected, actual) {
                (PublicSpanData::LocalSpan(expected), PublicSpanData::LocalSpan(actual)) => {
                    ensure!(
                        (
                            &expected.span_id,
                            expected.start,
                            &expected.parent_id,
                            expected.linked_context,
                            expected.inherited,
                        ) == (
                            &actual.span_id,
                            actual.start,
                            &actual.parent_id,
                            actual.linked_context,
                            actual.inherited,
                        ),
                        "recorded span facts changed"
                    );
                    assert_attributes(&expected.attributes, &actual.attributes)?;
                }
                _ => ensure!(expected == actual, "recorded external span changed"),
            }
        }
    }
    Ok(())
}

fn invocation_context(invocation: &PublicAgentInvocation) -> Option<&[Vec<PublicSpanData>]> {
    match invocation {
        PublicAgentInvocation::AgentInitialization(call) => Some(&call.invocation_context),
        PublicAgentInvocation::AgentMethodInvocation(call) => Some(&call.invocation_context),
        PublicAgentInvocation::ExternalTool(call) => Some(&call.invocation_context),
        _ => None,
    }
}

fn assert_span_projection(
    recorded: &SpanStarted,
    public: &PublicSpanStarted,
) -> anyhow::Result<()> {
    ensure!(
        (
            &recorded.span_id,
            &recorded.trace_id,
            &recorded.trace_states,
            &recorded.parent_span_id,
            recorded.started_at,
        ) == (
            &public.span_id,
            &public.trace_id,
            &public.trace_states,
            &public.parent_span_id,
            public.started_at,
        ),
        "span opening facts changed"
    );
    let kind = match &recorded.kind {
        SpanKind::Internal => PublicSpanKind::Internal,
        SpanKind::Client => PublicSpanKind::Client,
        SpanKind::Server => PublicSpanKind::Server,
    };
    ensure!(kind == public.kind && recorded.links.len() == public.links.len());
    for (expected, actual) in recorded.links.iter().zip(&public.links) {
        ensure!(
            (
                &expected.trace_id,
                &expected.span_id,
                &expected.trace_states
            ) == (&actual.trace_id, &actual.span_id, &actual.trace_states)
        );
    }
    assert_attributes(
        &recorded_attributes(&recorded.attributes),
        &public.attributes,
    )
}

fn validate_public_projection(snapshot: &OplogSnapshot) -> anyhow::Result<()> {
    ensure!(
        snapshot.raw.len() >= snapshot.public.len(),
        "public tip exceeds stored tip"
    );
    for (offset, public) in snapshot.public.iter().enumerate() {
        ensure!(
            public.oplog_index == record_index(offset),
            "public index continuity changed"
        );
        match (&snapshot.raw[offset], &public.entry) {
            (OplogEntry::Start { span_started, .. }, PublicOplogEntry::Start(call)) => {
                match (span_started, &call.span_started) {
                    (Some(recorded), Some(public)) => assert_span_projection(recorded, public)?,
                    (None, None) => {}
                    _ => anyhow::bail!("span opening presence changed"),
                }
            }
            (
                OplogEntry::End {
                    span_attributes, ..
                },
                PublicOplogEntry::End(call),
            ) => match (span_attributes, &call.span_attributes) {
                (Some(recorded), Some(public)) => {
                    ensure!(recorded.span_id == public.span_id);
                    assert_attributes(
                        &recorded_attributes(&recorded.attributes),
                        &public.attributes,
                    )?;
                }
                (None, None) => {}
                _ => anyhow::bail!("span attribute update presence changed"),
            },
            (
                OplogEntry::AgentInvocationStarted {
                    invocation_context: recorded,
                    ..
                },
                PublicOplogEntry::AgentInvocationStarted(call),
            ) => {
                if let Some(public) = invocation_context(&call.invocation) {
                    assert_context_projection(recorded, public)?;
                }
            }
            (
                OplogEntry::PendingAgentInvocation {
                    invocation_context: recorded,
                    ..
                },
                PublicOplogEntry::PendingAgentInvocation(call),
            ) => {
                if let Some(public) = invocation_context(&call.invocation) {
                    assert_context_projection(recorded, public)?;
                }
            }
            (
                OplogEntry::Start { .. }
                | OplogEntry::End { .. }
                | OplogEntry::AgentInvocationStarted { .. }
                | OplogEntry::PendingAgentInvocation { .. },
                _,
            ) => {
                anyhow::bail!("public entry kind differs from the recorded entry");
            }
            _ => {}
        }
    }
    Ok(())
}

fn assert_invocation_facts_equal(
    expected: &PublicAgentInvocation,
    actual: &PublicAgentInvocation,
) -> anyhow::Result<()> {
    // Contexts are fully checked against the recorded span producer for each snapshot.
    match (expected, actual) {
        (
            PublicAgentInvocation::AgentInitialization(expected),
            PublicAgentInvocation::AgentInitialization(actual),
        ) => {
            ensure!(
                (
                    &expected.idempotency_key,
                    &expected.constructor_parameters,
                    &expected.trace_id,
                    &expected.trace_states,
                ) == (
                    &actual.idempotency_key,
                    &actual.constructor_parameters,
                    &actual.trace_id,
                    &actual.trace_states,
                )
            );
        }
        (
            PublicAgentInvocation::AgentMethodInvocation(expected),
            PublicAgentInvocation::AgentMethodInvocation(actual),
        ) => {
            ensure!(
                (
                    &expected.idempotency_key,
                    &expected.method_name,
                    &expected.function_input,
                    &expected.trace_id,
                    &expected.trace_states,
                ) == (
                    &actual.idempotency_key,
                    &actual.method_name,
                    &actual.function_input,
                    &actual.trace_id,
                    &actual.trace_states,
                )
            );
        }
        (
            PublicAgentInvocation::ExternalTool(expected),
            PublicAgentInvocation::ExternalTool(actual),
        ) => {
            ensure!(
                (
                    &expected.idempotency_key,
                    &expected.tool_name,
                    &expected.command_path,
                    &expected.input,
                    &expected.trace_id,
                    &expected.trace_states,
                ) == (
                    &actual.idempotency_key,
                    &actual.tool_name,
                    &actual.command_path,
                    &actual.input,
                    &actual.trace_id,
                    &actual.trace_states,
                )
            );
        }
        _ => ensure!(expected == actual),
    }
    Ok(())
}

fn assert_public_facts_equal(
    expected: &PublicOplogEntry,
    actual: &PublicOplogEntry,
) -> anyhow::Result<()> {
    match (expected, actual) {
        (PublicOplogEntry::Start(expected), PublicOplogEntry::Start(actual)) => {
            ensure!(
                (
                    expected.timestamp,
                    expected.parent_start_index,
                    &expected.function_name,
                    &expected.invocation_id,
                    expected.observational_owner,
                    &expected.request,
                    &expected.durable_function_type,
                ) == (
                    actual.timestamp,
                    actual.parent_start_index,
                    &actual.function_name,
                    &actual.invocation_id,
                    actual.observational_owner,
                    &actual.request,
                    &actual.durable_function_type,
                )
            );
        }
        (PublicOplogEntry::End(expected), PublicOplogEntry::End(actual)) => {
            ensure!(
                (
                    expected.timestamp,
                    expected.start_index,
                    &expected.response,
                    expected.forced_commit,
                    &expected.span_finished,
                ) == (
                    actual.timestamp,
                    actual.start_index,
                    &actual.response,
                    actual.forced_commit,
                    &actual.span_finished,
                )
            );
        }
        (
            PublicOplogEntry::AgentInvocationStarted(expected),
            PublicOplogEntry::AgentInvocationStarted(actual),
        ) => {
            ensure!(
                expected.timestamp == actual.timestamp && expected.wallet_pin == actual.wallet_pin
            );
            assert_invocation_facts_equal(&expected.invocation, &actual.invocation)?;
        }
        (
            PublicOplogEntry::PendingAgentInvocation(expected),
            PublicOplogEntry::PendingAgentInvocation(actual),
        ) => {
            ensure!(expected.timestamp == actual.timestamp);
            assert_invocation_facts_equal(&expected.invocation, &actual.invocation)?;
        }
        _ => ensure!(expected == actual),
    }
    Ok(())
}

// These records exercise the comparison oracle only. They are never submitted as
// an entity admission or replay history to an executor.
fn oracle_fixture() -> OplogSnapshot {
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::payload::host_functions::HostFunctionName;
    use golem_common::model::oplog::public_oplog_entry::{
        AgentInvocationStartedParams, CompletionDeliveredParams, EndParams, StartParams,
    };
    use golem_common::model::oplog::{AgentInitializationParameters, DurableFunctionType};
    use std::collections::HashMap;

    let timestamp = Timestamp::from(1234_u64);
    let span_id = SpanId::from_string("1").unwrap();
    let parent_id = SpanId::from_string("2").unwrap();
    let trace_id = TraceId::from_string("11111111111111111111111111111111").unwrap();
    let attributes = HashMap::from([
        ("agent_id".into(), AttributeValue::String("owner".into())),
        (
            "agent_parameters".into(),
            AttributeValue::String("name".into()),
        ),
        (
            "idempotency_key".into(),
            AttributeValue::String("oracle-key".into()),
        ),
        (
            "function_name".into(),
            AttributeValue::String("initialize".into()),
        ),
        (
            "name".into(),
            AttributeValue::String("invoke-exported-function".into()),
        ),
        ("agent_type".into(), AttributeValue::String("Owner".into())),
        (
            "invocation_kind".into(),
            AttributeValue::String("AgentInitialization".into()),
        ),
    ]);
    let context = vec![
        SpanData::LocalSpan {
            span_id: span_id.clone(),
            start: timestamp,
            parent_id: Some(parent_id.clone()),
            linked_context: None,
            attributes: attributes.clone(),
            inherited: false,
        },
        SpanData::ExternalSpan { span_id: parent_id },
    ];
    let input = data_value!("first", "second");
    let payload = AgentInvocationPayload::AgentInitialization {
        input: input.value().clone(),
        principal: Principal::anonymous(),
    };
    let key = IdempotencyKey {
        value: "oracle-key".into(),
    };
    let token = WalletVersionToken {
        wallet_id_hash: [3; 32],
        generation: 4,
    };
    let span = SpanStarted {
        span_id: SpanId::from_string("3").unwrap(),
        trace_id: trace_id.clone(),
        trace_states: vec!["trace-state".into()],
        parent_span_id: None,
        links: Vec::new(),
        started_at: timestamp,
        attributes: AttributeMap(attributes),
        kind: SpanKind::Internal,
    };
    let update = SpanAttributes {
        span_id: SpanId::from_string("3").unwrap(),
        attributes: AttributeMap(HashMap::from([
            ("bytes".into(), AttributeValue::String("4".into())),
            ("result".into(), AttributeValue::String("known".into())),
        ])),
    };
    let raw = vec![
        OplogEntry::AgentInvocationStarted {
            timestamp,
            idempotency_key: key.clone(),
            payload: OplogPayload::Inline(Box::new(payload.clone())),
            trace_id: trace_id.clone(),
            trace_states: Vec::new(),
            invocation_context: context.clone(),
            wallet_pin: Box::new(InvocationWalletPin {
                wallet_token: token.clone(),
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            }),
        },
        OplogEntry::Start {
            timestamp,
            parent_start_index: Some(record_index(0)),
            function_name: HostFunctionName::Custom("oracle-scope".into()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: Some(Box::new(span.clone())),
        },
        OplogEntry::End {
            timestamp,
            start_index: record_index(1),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: Some(update.clone()),
        },
        OplogEntry::CompletionDelivered {
            timestamp,
            start_index: record_index(1),
        },
    ];
    let entries = vec![
        PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
            timestamp,
            invocation: PublicAgentInvocation::AgentInitialization(AgentInitializationParameters {
                idempotency_key: key,
                constructor_parameters: input,
                trace_id,
                trace_states: Vec::new(),
                invocation_context: encode_span_data(&context),
            }),
            wallet_pin: PublicInvocationWalletPin {
                wallet_token: token,
                scope_card_id: None,
            },
        }),
        PublicOplogEntry::Start(StartParams {
            timestamp,
            parent_start_index: Some(record_index(0)),
            function_name: "oracle-scope".into(),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal.into(),
            span_started: Some(PublicSpanStarted {
                span_id: span.span_id,
                trace_id: span.trace_id,
                trace_states: span.trace_states,
                parent_span_id: span.parent_span_id,
                links: Vec::new(),
                started_at: timestamp,
                attributes: recorded_attributes(&span.attributes),
                kind: PublicSpanKind::Internal,
            }),
        }),
        PublicOplogEntry::End(EndParams {
            timestamp,
            start_index: record_index(1),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: Some(PublicSpanAttributes {
                span_id: update.span_id,
                attributes: recorded_attributes(&update.attributes),
            }),
        }),
        PublicOplogEntry::CompletionDelivered(CompletionDeliveredParams {
            timestamp,
            start_index: record_index(1),
        }),
    ];
    OplogSnapshot {
        raw,
        public: entries
            .into_iter()
            .enumerate()
            .map(|(offset, entry)| PublicOplogEntryWithIndex {
                oplog_index: record_index(offset),
                attribution: PublicOplogEntryAttribution::agent(),
                entry,
            })
            .collect(),
        payloads: vec![(
            record_index(0),
            RecordedPayload::Invocation(Box::new(payload)),
        )],
    }
}

fn oracle_public_attributes(snapshot: &mut OplogSnapshot) -> &mut Vec<PublicAttribute> {
    let PublicOplogEntry::AgentInvocationStarted(call) = &mut snapshot.public[0].entry else {
        panic!("oracle fixture invocation");
    };
    let PublicAgentInvocation::AgentInitialization(call) = &mut call.invocation else {
        panic!("oracle fixture constructor");
    };
    let PublicSpanData::LocalSpan(span) = &mut call.invocation_context[0][0] else {
        panic!("oracle fixture local span");
    };
    &mut span.attributes
}

macro_rules! oracle_negative_case {
    ($name:ident, $change:expr) => {
        #[test]
        fn $name() {
            let expected = oracle_fixture();
            assert_prefix(&expected, &expected).unwrap();
            let mut actual = expected.clone();
            let change: fn(&mut OplogSnapshot) = $change;
            change(&mut actual);
            assert!(assert_prefix(&expected, &actual).is_err());
        }
    };
}

#[test]
fn oracle_accepts_only_span_map_iteration_changes() {
    let expected = oracle_fixture();
    assert_prefix(&expected, &expected).unwrap();
    let mut actual = expected.clone();
    oracle_public_attributes(&mut actual).rotate_left(1);
    let PublicOplogEntry::Start(call) = &mut actual.public[1].entry else {
        unreachable!()
    };
    call.span_started.as_mut().unwrap().attributes.reverse();
    let PublicOplogEntry::End(call) = &mut actual.public[2].entry else {
        unreachable!()
    };
    call.span_attributes.as_mut().unwrap().attributes.reverse();
    assert_snapshots_equal(&expected, &actual).unwrap();
}

oracle_negative_case!(oracle_rejects_recorded_attribute_value_change, |snapshot| {
    let OplogEntry::AgentInvocationStarted {
        invocation_context, ..
    } = &mut snapshot.raw[0]
    else {
        unreachable!()
    };
    let SpanData::LocalSpan { attributes, .. } = &mut invocation_context[0] else {
        unreachable!()
    };
    attributes.insert(
        "agent_id".into(),
        AttributeValue::String("changed-owner".into()),
    );
    oracle_public_attributes(snapshot)
        .iter_mut()
        .find(|attribute| attribute.key == "agent_id")
        .unwrap()
        .value = PublicAttributeValue::String(StringAttributeValue {
        value: "changed-owner".into(),
    });
});

oracle_negative_case!(oracle_rejects_recorded_attribute_removal, |snapshot| {
    let OplogEntry::AgentInvocationStarted {
        invocation_context, ..
    } = &mut snapshot.raw[0]
    else {
        unreachable!()
    };
    let SpanData::LocalSpan { attributes, .. } = &mut invocation_context[0] else {
        unreachable!()
    };
    attributes.remove("agent_id");
    oracle_public_attributes(snapshot).retain(|attribute| attribute.key != "agent_id");
});

oracle_negative_case!(oracle_rejects_recorded_span_id_change, |snapshot| {
    let OplogEntry::Start { span_started, .. } = &mut snapshot.raw[1] else {
        unreachable!()
    };
    span_started.as_mut().unwrap().span_id = SpanId::from_string("9").unwrap();
    let PublicOplogEntry::Start(call) = &mut snapshot.public[1].entry else {
        unreachable!()
    };
    call.span_started.as_mut().unwrap().span_id = SpanId::from_string("9").unwrap();
});

oracle_negative_case!(oracle_rejects_recorded_timestamp_change, |snapshot| {
    let OplogEntry::AgentInvocationStarted { timestamp, .. } = &mut snapshot.raw[0] else {
        unreachable!()
    };
    *timestamp = golem_common::model::Timestamp::from(1235_u64);
    let PublicOplogEntry::AgentInvocationStarted(call) = &mut snapshot.public[0].entry else {
        unreachable!()
    };
    call.timestamp = golem_common::model::Timestamp::from(1235_u64);
});

oracle_negative_case!(oracle_rejects_removed_record, |snapshot| {
    snapshot.raw.pop();
    snapshot.public.pop();
});

oracle_negative_case!(oracle_rejects_reordered_records, |snapshot| {
    snapshot.raw.swap(1, 2);
    snapshot.public.swap(1, 2);
});

oracle_negative_case!(oracle_rejects_changed_root_anchor, |snapshot| {
    let OplogEntry::Start {
        parent_start_index, ..
    } = &mut snapshot.raw[1]
    else {
        unreachable!()
    };
    *parent_start_index = Some(OplogIndex::from_u64(99));
    let PublicOplogEntry::Start(call) = &mut snapshot.public[1].entry else {
        unreachable!()
    };
    call.parent_start_index = Some(OplogIndex::from_u64(99));
});

oracle_negative_case!(oracle_rejects_changed_marker, |snapshot| {
    let OplogEntry::CompletionDelivered { start_index, .. } = &mut snapshot.raw[3] else {
        unreachable!()
    };
    *start_index = record_index(0);
    let PublicOplogEntry::CompletionDelivered(call) = &mut snapshot.public[3].entry else {
        unreachable!()
    };
    call.start_index = record_index(0);
});

oracle_negative_case!(oracle_rejects_changed_hydrated_payload, |snapshot| {
    let RecordedPayload::Invocation(payload) = &mut snapshot.payloads[0].1 else {
        unreachable!()
    };
    let AgentInvocationPayload::AgentInitialization { input, .. } = payload.as_mut() else {
        unreachable!()
    };
    *input = SchemaValue::String("different payload under the same raw reference".into());
});

oracle_negative_case!(oracle_rejects_changed_payload_reference, |snapshot| {
    let OplogEntry::AgentInvocationStarted { payload, .. } = &mut snapshot.raw[0] else {
        unreachable!()
    };
    *payload = OplogPayload::SerializedInline {
        bytes: vec![1, 2, 3],
        cached: None,
    };
});

oracle_negative_case!(oracle_rejects_public_duplicate_attribute_key, |snapshot| {
    let attributes = oracle_public_attributes(snapshot);
    attributes[1] = attributes[0].clone();
});

oracle_negative_case!(
    oracle_rejects_public_attribute_cardinality_change,
    |snapshot| {
        oracle_public_attributes(snapshot).pop();
    }
);

oracle_negative_case!(
    oracle_rejects_public_wrong_typed_attribute_value,
    |snapshot| {
        oracle_public_attributes(snapshot)[0].value =
            PublicAttributeValue::String(StringAttributeValue {
                value: "not the recorded typed value".into(),
            });
    }
);

oracle_negative_case!(
    oracle_rejects_public_opening_duplicate_attribute_key,
    |snapshot| {
        let PublicOplogEntry::Start(call) = &mut snapshot.public[1].entry else {
            unreachable!()
        };
        let attributes = &mut call.span_started.as_mut().unwrap().attributes;
        attributes[1] = attributes[0].clone();
    }
);

oracle_negative_case!(
    oracle_rejects_public_update_wrong_typed_attribute_value,
    |snapshot| {
        let PublicOplogEntry::End(call) = &mut snapshot.public[2].entry else {
            unreachable!()
        };
        call.span_attributes.as_mut().unwrap().attributes[0].value =
            PublicAttributeValue::String(StringAttributeValue {
                value: "changed".into(),
            });
    }
);

oracle_negative_case!(oracle_rejects_public_span_nonmap_fact_change, |snapshot| {
    let PublicOplogEntry::Start(call) = &mut snapshot.public[1].entry else {
        unreachable!()
    };
    call.span_started
        .as_mut()
        .unwrap()
        .trace_states
        .push("unrecorded".into());
});

oracle_negative_case!(oracle_rejects_ordered_guest_input_change, |snapshot| {
    let PublicOplogEntry::AgentInvocationStarted(call) = &mut snapshot.public[0].entry else {
        unreachable!()
    };
    let PublicAgentInvocation::AgentInitialization(call) = &mut call.invocation else {
        unreachable!()
    };
    call.constructor_parameters = data_value!("second", "first");
});

oracle_negative_case!(oracle_rejects_span_stack_reordering, |snapshot| {
    let PublicOplogEntry::AgentInvocationStarted(call) = &mut snapshot.public[0].entry else {
        unreachable!()
    };
    let PublicAgentInvocation::AgentInitialization(call) = &mut call.invocation else {
        unreachable!()
    };
    call.invocation_context[0].swap(0, 1);
});

oracle_negative_case!(oracle_rejects_public_index_discontinuity, |snapshot| {
    snapshot.public[2].oplog_index = OplogIndex::from_u64(99);
});

oracle_negative_case!(oracle_rejects_public_attribution_change, |snapshot| {
    snapshot.public[0].attribution = PublicOplogEntryAttribution::Entity(
        golem_common::model::oplog::PublicEntityInvocationContext {
            invocation: golem_common::model::oplog::PublicEntityInvocation {
                entity: golem_common::model::oplog::PublicAgentEntity {
                    kind: PublicAgentEntityKind::Tool,
                    name: "wrong entity".into(),
                },
                start_index: record_index(0),
                call_mode: PublicEntityCallMode::Synchronous,
                operation: None,
            },
            ancestors: Vec::new(),
        },
    );
});

#[test]
fn oracle_rejects_late_settlement_record() {
    let expected = oracle_fixture();
    assert_prefix(&expected, &expected).unwrap();
    let mut actual = expected.clone();
    actual.raw.push(OplogEntry::CompletionDelivered {
        timestamp: golem_common::model::Timestamp::from(1234_u64),
        start_index: record_index(1),
    });
    assert!(assert_snapshots_equal(&expected, &actual).is_err());
}

#[test]
fn oracle_rejects_payload_checksum_change() {
    let mut expected = oracle_fixture();
    let OplogEntry::AgentInvocationStarted { payload, .. } = &mut expected.raw[0] else {
        unreachable!()
    };
    *payload = OplogPayload::External {
        payload_id: golem_common::model::oplog::PayloadId(uuid::Uuid::from_u128(1)),
        md5_hash: vec![0; 16],
        cached: None,
    };
    assert_prefix(&expected, &expected).unwrap();
    let mut actual = expected.clone();
    let OplogEntry::AgentInvocationStarted {
        payload: OplogPayload::External { md5_hash, .. },
        ..
    } = &mut actual.raw[0]
    else {
        unreachable!()
    };
    md5_hash[0] = 1;
    assert!(assert_prefix(&expected, &actual).is_err());
}
