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

use crate::Tracing;
use futures::poll;
use golem_common::model::agent::{AgentMode, AgentPrincipal, Principal};
use golem_common::model::durable_stream::*;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{
    AgentId, AgentInvocationPayload, AgentInvocationResult, IdempotencyKey, OwnedAgentId, Timestamp,
};
use golem_common::{agent_id, data_value};
use golem_schema::schema::SchemaFingerprintV1;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::oplog::OplogOps;
use golem_worker_executor::services::{
    HasActiveAgents, HasOplog, HasOplogService, HasRpc, HasWorkerService, UsesAllDeps,
};
use golem_worker_executor::storage::keyvalue::KeyValueStorageError;
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::worker::Worker;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerCtx,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

const CACHE_TTL: Duration = Duration::from_millis(50);

async fn setup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
) -> anyhow::Result<(
    TestWorkerExecutor,
    Arc<Worker<TestWorkerCtx>>,
    KeyValueStorageFaults,
)> {
    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.active_agents.ttl = CACHE_TTL;
                config.oplog.max_payload_size = 1;
                config.durable_stream.renewal_interval = Duration::from_millis(50);
                config.durable_stream.reconciliation_interval = Duration::from_millis(50);
                config.agent_status_flush.enabled = false;
            })),
            wrap_key_value_storage: Some(Arc::new({
                let faults = faults.clone();
                move |storage| Arc::new(FaultInjectingKeyValueStorage::new(storage, faults.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_id!("Counter", "initialization-test-seed");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &name, "increment", data_value!())
        .await?;
    let seed = executor
        .active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .unwrap()
        .primary();
    tokio::time::timeout(Duration::from_secs(20), async {
        while executor
            .worker_is_loaded(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
        {
            seed.stop_if_idle().await;
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok((executor, seed, faults))
}

fn target(seed: &Worker<TestWorkerCtx>, name: &str) -> OwnedAgentId {
    OwnedAgentId::new(
        seed.get_initial_worker_metadata().environment_id,
        &AgentId {
            component_id: seed.agent_id().component_id,
            agent_id: agent_id!("Counter", name.to_string()).to_string(),
        },
    )
}

async fn acquire(
    seed: &Arc<Worker<TestWorkerCtx>>,
    target: &OwnedAgentId,
) -> Result<Arc<Worker<TestWorkerCtx>>, WorkerExecutorError> {
    Worker::get_or_create_suspended(
        seed.all(),
        target,
        Some(vec![("OWNER".into(), "original".into())]),
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await
}

async fn register_stream(worker: &Worker<TestWorkerCtx>) -> anyhow::Result<DurableStreamHandle> {
    let metadata = worker.get_initial_worker_metadata();
    let index = worker.oplog().current_oplog_index().await.next();
    let source_invocation = StreamInvocationId {
        callee_environment_id: metadata.environment_id,
        callee: metadata.agent_id.clone(),
        callee_fingerprint: metadata.fingerprint,
        idempotency_key: IdempotencyKey::new("stream-session".into()),
    };
    let handle = DurableStreamHandle {
        format_version: 1,
        stream_id: StreamId::derive(
            metadata.environment_id,
            &metadata.agent_id,
            metadata.fingerprint,
            index,
        )?,
        producer_environment_id: metadata.environment_id,
        producer: metadata.agent_id,
        expected_producer_fingerprint: metadata.fingerprint,
        source_invocation: source_invocation.clone(),
        component_revision: metadata.last_known_status.component_revision,
        element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
    };
    assert_eq!(
        worker
            .add_and_commit_oplog(OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamRegisteredRecord {
                    format_version: 1,
                    coordinate: StreamRegistrationCoordinate::Root {
                        invocation_id: source_invocation,
                        root_kind: StreamRootKind::MethodResult,
                        recursive_value_path: vec![],
                    },
                    registration_oplog_index: index,
                    handle: handle.clone(),
                    source_kind: StreamSourceKind::InvocationOutput,
                    session_mapping: None,
                })),
            })
            .await,
        index
    );
    Ok(handle)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn late_initialization_storage_failure_is_shared_then_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let id = target(&seed, "late-storage-failure");
    let original = acquire(&seed, &id).await?;
    seed.active_agents().remove(&id).await;
    register_stream(&original).await?;
    let before = original.get_initial_worker_metadata();
    let status = faults.gate_next(
        "update_status",
        KeyValueStorageError::Other("old status write outage".into()),
    );
    let write = tokio::spawn({
        let original = original.clone();
        async move {
            original
                .add_and_commit_oplog(OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                })
                .await
        }
    });
    status.entered().await;
    write.abort();
    assert!(write.await.unwrap_err().is_cancelled());
    drop(original);
    let failure = faults.gate_next(
        "lookup_producer",
        KeyValueStorageError::Other("late construction outage".into()),
    );
    let mut first = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut first).is_pending());
    failure.entered().await;
    assert!(!executor.worker_is_cached(&id).await);
    let mut waiters = vec![first];
    for _ in 0..3 {
        let mut waiter = Box::pin(acquire(&seed, &id));
        assert!(poll!(&mut waiter).is_pending());
        waiters.push(waiter);
    }
    failure.release();
    // Cancellation of the writer's caller must not let construction finish cleanup before
    // the old generation's actual status job completes.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut waiters[0])
            .await
            .is_err()
    );
    let mut waiter = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut waiter).is_pending());
    waiters.push(waiter);
    status.release();
    let mut errors = Vec::new();
    for waiter in waiters {
        errors.push(waiter.await.err().expect("all original waiters must fail"));
    }
    assert!(errors[0].to_string().contains("late construction outage"));
    assert!(errors.iter().all(|error| error == &errors[0]));
    assert!(!executor.worker_is_cached(&id).await);

    let (retry, overlapping) = tokio::join!(acquire(&seed, &id), acquire(&seed, &id));
    let retry = retry?;
    assert!(Arc::ptr_eq(&retry, &overlapping?));
    let after = retry.get_initial_worker_metadata();
    assert_eq!(before.fingerprint, after.fingerprint);
    assert_eq!(before.env, after.env);
    assert_eq!(before.config, after.config);
    assert_eq!(
        before.last_known_status.component_revision,
        after.last_known_status.component_revision
    );
    let entries = retry
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            retry.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    assert_eq!(retry.pending_invocations().await.len(), 1);
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn partial_creation_reloads_identity_and_original_initialization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let all = seed.all().clone();
    for agent_type in ["Counter", "EphemeralCounter"] {
        let mut id = target(&seed, "partial-creation");
        id.agent_id.agent_id = agent_id!(agent_type, "partial-creation").to_string();
        faults.fail(
            "read_cached_agent_mode",
            1,
            KeyValueStorageError::Other("before creation outage".into()),
        );
        assert!(
            acquire(&seed, &id)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("before creation outage")
        );
        assert!(Worker::get_latest_metadata(&all, &id).await?.is_none());
        let context = InvocationContextStack::fresh();
        let principal = Principal::Agent(AgentPrincipal {
            agent_id: seed.agent_id(),
        });
        let payload_directory = deps
            .blob_storage_root()
            .join("oplog_payload")
            .join("ephemeral")
            .join(id.environment_id.to_string())
            .join(FileSystemBlobStorage::filesystem_safe_oplog_payload_agent_key(&id.agent_id));
        let expected_error = if agent_type == "Counter" {
            faults.fail(
                "update_status",
                1,
                KeyValueStorageError::Other("initial status outage".into()),
            );
            "initial status outage"
        } else {
            tokio::fs::create_dir_all(payload_directory.parent().unwrap()).await?;
            tokio::fs::write(&payload_directory, b"block payload directory creation").await?;
            "Failed to upload invocation payload"
        };
        let failure = Worker::get_or_create_suspended(
            &all,
            &id,
            Some(vec![("OWNER".into(), "first-creator".into())]),
            vec![],
            None,
            None,
            &context,
            principal.clone(),
        )
        .await
        .err()
        .expect("creation must report the injected failure");
        assert!(failure.to_string().contains(expected_error));
        assert!(!executor.worker_is_cached(&id).await);
        let before = Worker::get_latest_metadata(&all, &id).await?.unwrap();
        assert_eq!(before.last_known_status.oplog_idx, OplogIndex::INITIAL);
        let sentinel_id = target(&seed, &format!("expiry-sentinel-{agent_type}"));
        let sentinel = acquire(&seed, &sentinel_id).await?;
        let sentinel_weak = Arc::downgrade(&sentinel);
        drop(sentinel);
        tokio::time::timeout(Duration::from_secs(20), async {
            while sentinel_weak.upgrade().is_some() {
                tokio::time::sleep(CACHE_TTL).await;
            }
        })
        .await?;
        if agent_type == "EphemeralCounter" {
            tokio::fs::remove_file(&payload_directory).await?;
        }

        // A different caller retries. Create and initialization provenance belong to the first one.
        let worker = acquire(&seed, &id).await?;
        let after = worker.get_initial_worker_metadata();
        assert_eq!(before.fingerprint, after.fingerprint);
        assert_eq!(before.env, after.env);
        assert_eq!(before.config, after.config);
        assert_eq!(
            before.last_known_status.component_revision,
            after.last_known_status.component_revision
        );
        let oplog = worker.oplog();
        let entries = oplog
            .read_exact(
                OplogIndex::INITIAL,
                oplog.current_oplog_index().await.as_u64(),
            )
            .await;
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
                .count(),
            1
        );
        let pending = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::PendingAgentInvocation {
                    payload, trace_id, ..
                } => Some((payload, trace_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        let AgentInvocationPayload::AgentInitialization {
            principal: recorded,
            ..
        } = oplog
            .download_payload(pending[0].0.clone())
            .await
            .map_err(anyhow::Error::msg)?
        else {
            panic!("expected initialization payload")
        };
        assert_eq!(recorded, principal);
        assert_eq!(pending[0].1, &context.trace_id);
        if agent_type == "EphemeralCounter" {
            // No guest was admitted by the failed creation, so this is not a fail-stop reload.
            Worker::start_if_needed(worker.clone()).await?;
        }
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn cancelled_creator_keeps_original_context_and_serializes_existing_only(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let id = target(&seed, "cancelled-creator");
    let all = seed.all().clone();
    Worker::interrupt(&all, &id, false, Principal::anonymous()).await?;
    assert!(!executor.worker_is_cached(&id).await);
    assert!(!seed.oplog_service().exists(&id, AgentMode::Durable).await);
    let mut enqueue = executor
        .gate_next_agent_initialization_enqueue(&id.agent_id)
        .await;
    let context = InvocationContextStack::fresh();
    let original_principal = Principal::Agent(AgentPrincipal {
        agent_id: seed.agent_id(),
    });
    let creating = tokio::spawn({
        let all = all.clone();
        let id = id.clone();
        let context = context.clone();
        let principal = original_principal.clone();
        async move {
            Worker::get_or_create_suspended(
                &all,
                &id,
                Some(vec![("OWNER".into(), "creator".into())]),
                vec![],
                None,
                None,
                &context,
                principal,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), enqueue.entered())
        .await
        .expect("creator must reach the initialization enqueue");
    creating.abort();
    assert!(creating.await.err().unwrap().is_cancelled());
    let mut existing = Box::pin(Worker::interrupt(&all, &id, false, Principal::anonymous()));
    let mut competing = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut existing).is_pending());
    assert!(poll!(&mut competing).is_pending());
    assert!(!executor.worker_is_cached(&id).await);
    drop(enqueue);
    let (existing, worker) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(existing, competing)
    })
    .await
    .expect("acquisitions must finish after initialization enqueue resumes");
    existing?;
    let worker = worker?;
    assert_eq!(
        worker.get_initial_worker_metadata().env,
        vec![("OWNER".into(), "creator".into())]
    );
    let oplog = worker.oplog();
    let entries = oplog
        .read_exact(
            OplogIndex::INITIAL,
            oplog.current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    let mut initializations = 0;
    for entry in entries.values() {
        if let OplogEntry::PendingAgentInvocation {
            payload, trace_id, ..
        } = entry
            && let AgentInvocationPayload::AgentInitialization { principal, .. } = oplog
                .download_payload(payload.clone())
                .await
                .map_err(anyhow::Error::msg)?
        {
            initializations += 1;
            assert_eq!(principal, original_principal);
            assert_eq!(trace_id, &context.trace_id);
        }
    }
    assert_eq!(initializations, 1);
    Ok(())
}

async fn prepare_foreign_topology(
    worker: &Worker<TestWorkerCtx>,
    source: &DurableStreamHandle,
    consumer_invocation: StreamInvocationId,
    transport_stream_id: u64,
) -> anyhow::Result<(StreamAttachmentKey, StreamSessionMappingRecord)> {
    let metadata = worker.get_initial_worker_metadata();
    let session_key = source.source_invocation.clone();
    let attachment = StreamAttachmentKey {
        attachment_id: AttachmentId::primary(
            session_key.callee_environment_id,
            &session_key.callee,
            &session_key.idempotency_key,
        )?,
        stream_id: source.stream_id,
        epoch: 1,
        session_key: session_key.clone(),
        producer_environment_id: source.producer_environment_id,
        producer: source.producer.clone(),
        expected_producer_fingerprint: source.expected_producer_fingerprint,
        consumer_environment_id: metadata.environment_id,
        consumer: metadata.agent_id,
        expected_consumer_fingerprint: metadata.fingerprint,
        consumer_invocation,
    };
    let mapping = StreamSessionMappingRecord {
        transport_stream_id,
        handle: source.clone(),
        role: SessionStreamRole::Output,
    };
    worker
        .add_and_commit_oplog(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::TopologyPrepared(
                StreamTopologyPreparedRecord {
                    format_version: 1,
                    session_key,
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                },
            ))),
        })
        .await;
    Ok((attachment, mapping))
}

async fn session_records(
    worker: &Worker<TestWorkerCtx>,
) -> anyhow::Result<Vec<StreamSessionRecord>> {
    let oplog = worker.oplog();
    let entries = oplog
        .read_exact(
            OplogIndex::INITIAL,
            oplog.current_oplog_index().await.as_u64(),
        )
        .await;
    let mut result = Vec::new();
    for entry in entries.into_values() {
        if let OplogEntry::StreamSession { record, .. } = entry {
            result.push(
                oplog
                    .download_payload(record)
                    .await
                    .map_err(anyhow::Error::msg)?,
            );
        }
    }
    Ok(result)
}

async fn prepare_session(
    worker: &Worker<TestWorkerCtx>,
    completed: bool,
) -> anyhow::Result<StreamSessionKey> {
    let oplog = worker.oplog();
    let pending_index = oplog.current_oplog_index().await.next().next();
    let idempotency_key = IdempotencyKey::new("stream-session".into());
    let payload = OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
        method_name: "increment".into(),
        input: data_value!().value().clone(),
        principal: Principal::anonymous(),
        scope_card: None,
    }));
    let context = InvocationContextStack::fresh();
    let invocation_context = context.to_oplog_data();
    let trace_id = context.trace_id;
    let trace_states = context.trace_states;
    let metadata = worker.get_initial_worker_metadata();
    let session_key = StreamInvocationId {
        callee_environment_id: metadata.environment_id,
        callee: metadata.agent_id.clone(),
        callee_fingerprint: metadata.fingerprint,
        idempotency_key: idempotency_key.clone(),
    };
    let attachment_id = AttachmentId::primary(
        metadata.environment_id,
        &metadata.agent_id,
        &idempotency_key,
    )?;
    let attempt_id = AttemptId::fresh();
    for record in [
        StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
            format_version: 1,
            attempt: StartAttemptDescriptor {
                format_version: 1,
                session_key: session_key.clone(),
                attachment_id,
                expected_callee_fingerprint: metadata.fingerprint,
                attempt_id,
                invocation: PersistedStreamInvocationDescriptor {
                    format_version: 1,
                    session_key: session_key.clone(),
                    target_component_revision: metadata.last_known_status.component_revision,
                    method_name: "increment".into(),
                    invocation_value: vec![],
                    stream_handles: vec![],
                    execution_config: vec![],
                    effective_identity: vec![],
                },
                effective_identity: vec![],
                live_join_buffer_events: 1,
            },
            stream_mappings: vec![],
        }),
        StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id,
            attempt_id,
            epoch: 1,
            pending_invocation_oplog_index: pending_index,
        }),
    ] {
        let prepared = matches!(record, StreamSessionRecord::Prepared(_));
        worker
            .add_and_commit_oplog(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(record)),
            })
            .await;
        if prepared {
            assert_eq!(
                worker
                    .add_and_commit_oplog(OplogEntry::pending_agent_invocation(
                        idempotency_key.clone(),
                        payload.clone(),
                        trace_id.clone(),
                        trace_states.clone(),
                        invocation_context.clone(),
                    ))
                    .await,
                pending_index
            );
        }
    }
    if completed {
        worker
            .add_and_commit_oplog(OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                wallet_pin: None,
            })
            .await;
        worker
            .add_and_commit_oplog(OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentMethod {
                    output: data_value!(1u32).value().clone(),
                })),
                method_name: Some("increment".into()),
                consumed_fuel: 0,
                component_revision: metadata.last_known_status.component_revision,
            })
            .await;
    }
    Ok(session_key)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn reciprocal_cold_topologies_recover_without_initialization_cycle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let a_id = target(&seed, "reciprocal-a");
    let b_id = target(&seed, "reciprocal-b");
    let a = acquire(&seed, &a_id).await?;
    let b = acquire(&seed, &b_id).await?;
    seed.active_agents().remove(&a_id).await;
    seed.active_agents().remove(&b_id).await;
    let a_stream = register_stream(&a).await?;
    let b_stream = register_stream(&b).await?;
    let (a_attachment, a_mapping) =
        prepare_foreign_topology(&a, &b_stream, a_stream.source_invocation.clone(), 11).await?;
    let a_fingerprint = a.get_initial_worker_metadata().fingerprint;
    let (b_attachment, _) =
        prepare_foreign_topology(&b, &a_stream, b_stream.source_invocation.clone(), 29).await?;
    let completed_session = prepare_session(&a, true).await?;
    prepare_session(&b, false).await?;
    drop(a);
    drop(b);
    assert!(!executor.worker_is_cached(&a_id).await);
    assert!(!executor.worker_is_cached(&b_id).await);

    seed.worker_service()
        .lookup_durable_stream_recovery_metadata(&a_id, AgentMode::Durable, a_fingerprint)
        .await
        .map_err(anyhow::Error::msg)?;
    // Hold A's recovery after publication. B may acquire A while recovering its own attachment,
    // but a stream read may not treat A's merely prepared topology as an active attachment.
    let recovery = faults.gate_next(
        "read_recovery",
        KeyValueStorageError::Other("recovery storage outage".into()),
    );
    let a = tokio::time::timeout(Duration::from_secs(20), acquire(&seed, &a_id)).await??;
    recovery.entered().await;
    assert!(executor.worker_is_cached(&a_id).await);
    let b = tokio::time::timeout(Duration::from_secs(20), acquire(&seed, &b_id)).await??;
    let read = seed
        .rpc()
        .read_durable_stream_segment(
            DurableStreamReadRequest::AttachedConsumer(Box::new(AttachedStreamSegmentRequest {
                format_version: 1,
                attachment: a_attachment.clone(),
                mapping: a_mapping,
                after: None,
                through: None,
                wait_for_events: false,
            })),
            &AuthCtx::System,
        )
        .await;
    assert!(
        read.is_err(),
        "prepared topology must not authorize a stream read"
    );
    let before = session_records(&a).await?;
    assert!(!before.iter().any(|record| matches!(
        record,
        StreamSessionRecord::TopologyActivated(_) | StreamSessionRecord::Finished(_)
    )));
    recovery.release();

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let a_records = session_records(&a).await?;
            let b_records = session_records(&b).await?;
            let a_activated = a_records.iter().filter(|record| matches!(record, StreamSessionRecord::TopologyActivated(record) if record.attachment == a_attachment)).count();
            let b_activated = b_records.iter().filter(|record| matches!(record, StreamSessionRecord::TopologyActivated(record) if record.attachment == b_attachment)).count();
            let finished = a_records.iter().filter(|record| matches!(record, StreamSessionRecord::Finished(record) if record.session_key == completed_session)).count();
            if a_activated != 0 && b_activated != 0 && finished != 0 {
                assert_eq!(a_activated, 1);
                assert_eq!(b_activated, 1);
                assert_eq!(finished, 1);
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await??;
    assert_eq!(
        a.get_initial_worker_metadata().fingerprint,
        a_stream.expected_producer_fingerprint
    );
    assert_eq!(
        b.get_initial_worker_metadata().fingerprint,
        b_stream.expected_producer_fingerprint
    );
    Ok(())
}
