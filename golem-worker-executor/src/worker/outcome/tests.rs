use super::*;
use crate::durable_host::durable_stream::tests::TestOplog;
use crate::services::oplog::{Oplog, OplogReadError};
use crate::worker::interrupt::Interrupts;
use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt, UnloadReason, UnloadRequest};
use golem_common::model::Timestamp;
use golem_common::model::oplog::payload::OplogPayload;
use test_r::test;

struct ResultServices(Arc<TestOplog>);

impl HasOplog for ResultServices {
    fn oplog(&self) -> Arc<dyn Oplog> {
        self.0.clone()
    }
}

impl HasOplogService for ResultServices {
    fn oplog_service(&self) -> Arc<dyn crate::services::oplog::OplogService> {
        panic!("finished results need no service read")
    }
}

impl HasConfig for ResultServices {
    fn config(&self) -> Arc<GolemConfig> {
        panic!("finished results need no config")
    }
}

impl HasComponentService for ResultServices {
    fn component_service(&self) -> Arc<dyn crate::services::component::ComponentService> {
        panic!("finished results need no component lookup")
    }
}

async fn lazy_result(
    payload: OplogPayload<AgentInvocationResult>,
) -> (
    ResultServices,
    InvocationResult,
    OplogIndex,
    OwnedAgentId,
    AgentFingerprint,
) {
    let services = ResultServices(Arc::new(TestOplog::default()));
    services.0.add(OplogEntry::suspend()).await.unwrap();
    let index = services
        .0
        .add(OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            invocation_id: InvocationId(uuid::Uuid::new_v4()),
            result: payload,
            method_name: Some("count".into()),
            consumed_fuel: 17,
            component_revision: ComponentRevision::INITIAL,
            awaits_settlement: false,
        })
        .await
        .unwrap();
    assert!(index > OplogIndex::INITIAL);
    let id = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &golem_common::model::AgentId::from_parts(7, 42),
    );
    (
        services,
        InvocationResult::Lazy {
            oplog_idx: index,
            background_failure: None,
        },
        index,
        id,
        AgentFingerprint(uuid::Uuid::new_v4()),
    )
}

#[test]
async fn lazy_invocation_result_returns_unknown_payload_version() {
    let (services, mut result, index, id, fingerprint) =
        lazy_result(OplogPayload::SerializedInline {
            bytes: [vec![255], b"private payload".to_vec()].concat(),
            cached: None,
        })
        .await;
    let error = result
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap_err();
    assert_eq!(error.agent_id, id.agent_id);
    assert!(matches!(error.error, OplogReadError::Corruption { .. }));
    let message = error.to_string();
    assert!(
        message.contains(&format!("oplog index {index}")),
        "{message}"
    );
    assert!(message.contains("version: 255"), "{message}");
    assert!(!message.contains("private payload"), "{message}");
    assert!(matches!(
        WorkerExecutorError::from(error),
        WorkerExecutorError::OplogFailed { .. }
    ));
    assert!(matches!(
        result,
        InvocationResult::Lazy { oplog_idx, background_failure: None } if oplog_idx == index
    ));
}

#[test]
async fn lazy_invocation_result_returns_missing_payload_version() {
    let (services, mut result, index, id, fingerprint) =
        lazy_result(OplogPayload::SerializedInline {
            bytes: vec![],
            cached: None,
        })
        .await;
    let error = result
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap_err();
    assert_eq!(error.agent_id, id.agent_id);
    assert!(matches!(error.error, OplogReadError::Corruption { .. }));
    let message = error.to_string();
    assert!(
        message.contains(&format!("oplog index {index}")),
        "{message}"
    );
    assert!(
        message.contains("missing serialization version"),
        "{message}"
    );
    assert!(matches!(
        WorkerExecutorError::from(error),
        WorkerExecutorError::OplogFailed { .. }
    ));
    assert!(matches!(
        result,
        InvocationResult::Lazy { oplog_idx, background_failure: None } if oplog_idx == index
    ));
}

#[test]
async fn lazy_invocation_result_preserves_temporary_payload_failure() {
    let (services, mut result, index, id, fingerprint) = lazy_result(OplogPayload::External {
        payload_id: golem_common::model::oplog::PayloadId(uuid::Uuid::new_v4()),
        md5_hash: vec![],
        cached: None,
    })
    .await;
    let error = result
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap_err();
    assert_eq!(error.agent_id, id.agent_id);
    assert!(matches!(
        error.error,
        OplogReadError::SourceUnavailable { .. }
    ));
    assert!(error.is_transient());
    let message = error.to_string();
    assert!(
        message.contains(&format!("oplog index {index}")),
        "{message}"
    );
    assert!(matches!(
        WorkerExecutorError::from(error),
        WorkerExecutorError::RecoveryRequired { .. }
    ));
    assert!(matches!(
        result,
        InvocationResult::Lazy { oplog_idx, background_failure: None } if oplog_idx == index
    ));
}

#[test]
async fn lazy_invocation_result_hydrates_a_valid_payload_once() {
    let invocation = AgentInvocationResult::AgentMethod {
        output: golem_common::schema::SchemaValue::U32(42),
    };
    let (services, mut result, index, id, fingerprint) =
        lazy_result(OplogPayload::SerializedInline {
            bytes: golem_common::serialization::serialize(&invocation).unwrap(),
            cached: None,
        })
        .await;
    result
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap();
    let InvocationResult::Cached { result: Ok(output) } = &result else {
        panic!("expected cached result");
    };
    assert_eq!(output.result, invocation);
    assert_eq!(output.consumed_fuel, Some(17));
    assert_eq!(output.component_revision, Some(ComponentRevision::INITIAL));
    assert_eq!(output.oplog_index, Some(index));
    assert_eq!(output.agent_fingerprint, Some(fingerprint));
    let expected = output.clone();
    services.0.make_reads_unavailable([index]);
    result
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap();
    assert!(
        matches!(result, InvocationResult::Cached { result: Ok(output) } if output == expected)
    );
}

#[test]
fn invocation_rejection_lookup_preserves_the_typed_error() {
    let key = InvocationId(uuid::Uuid::new_v4());
    let status = AgentStatusRecord::default();
    for (error, expected) in [
        (
            AgentError::InvalidRequest("method not found".to_string()),
            WorkerExecutorError::invalid_request("method not found"),
        ),
        (
            AgentError::PermissionDenied("not authorized".to_string()),
            WorkerExecutorError::permission_denied("not authorized"),
        ),
    ] {
        let actual = lookup_result_from_cached_result(
            &status,
            &key,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult::Trap {
                    trap_type: TrapType::Error {
                        error,
                        retry_from: OplogIndex::INITIAL,
                        semantic_trap_retry_override: None,
                        provenance: crate::model::FailureProvenance::Host,
                    },
                    stderr: String::new(),
                }),
            },
        );
        assert!(matches!(actual, LookupResult::Complete(Err(error)) if error == expected));
    }
    assert!(matches!(
        lookup_result_from_cached_result(&status, &key, InvocationResult::Cancelled),
        LookupResult::Complete(Err(WorkerExecutorError::InvocationCancelled))
    ));
}

#[test]
async fn a_stop_after_the_ordinary_terminal_check_remains_for_the_invocation_loop() {
    let interrupts = Interrupts::default();
    assert!(interrupts.claim_pending_terminal().await.is_none());
    let kind = InterruptKind::Suspend(Timestamp::from(63_u64));
    {
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
            establishment: InterruptEstablishment::ready(),
        }));
    }
    assert_eq!(interrupts.take().await.unwrap().kind, kind);
    assert!(interrupts.take().await.is_none());
}

#[test]
fn monthly_mapping_requires_exact_accepted_kind_and_ephemeral_mode() {
    let config = GolemConfig::default();
    let kind = InterruptKind::Suspend(Timestamp::from(50_u64));
    let other = InterruptKind::Suspend(Timestamp::from(51_u64));
    for exhaustion in [
        MonthlyResourceExhaustion::Compute,
        MonthlyResourceExhaustion::Memory,
        MonthlyResourceExhaustion::DurableStorage,
        MonthlyResourceExhaustion::EphemeralStorage,
    ] {
        let accepted = Some((kind, exhaustion));
        assert!(monthly_interrupt_error(kind, AgentMode::Durable, &accepted, &config).is_none());
        assert!(monthly_interrupt_error(other, AgentMode::Ephemeral, &accepted, &config).is_none());
        let actual =
            monthly_interrupt_error(kind, AgentMode::Ephemeral, &accepted, &config).unwrap();
        let expected = crate::worker::invocation_loop::monthly_resource_exhausted_invocation_error(
            &config, exhaustion,
        );
        assert_eq!(actual, expected);
        let trap = monthly_failure::<crate::workerctx::default::Context>(
            &TrapType::Interrupt(kind),
            AgentMode::Ephemeral,
            &accepted,
            &config,
            false,
            OplogIndex::from_u64(37),
        )
        .expect("monthly exhaustion converts the accepted suspension");
        assert!(
            crate::durable_host::ephemeral_suspend_trap(
                AgentMode::Ephemeral,
                &trap,
                OplogIndex::from_u64(37)
            )
            .is_none(),
            "the active-suspend fallback must not replace the monthly error"
        );
        assert_eq!(trap.as_golem_error(""), Some(expected));
    }
}

#[test]
fn monthly_policy_outage_preserves_its_reason_in_ephemeral_suspend_failure() {
    let config = GolemConfig::default();
    let retry_from = OplogIndex::from_u64(37);
    let suspend = TrapType::Interrupt(InterruptKind::Suspend(Timestamp::from(50_u64)));
    let converted = monthly_failure::<crate::workerctx::default::Context>(
        &suspend,
        AgentMode::Ephemeral,
        &None,
        &config,
        true,
        retry_from,
    )
    .expect("an active ephemeral policy outage is terminal");
    assert!(matches!(&converted,
        TrapType::Error {
            error: AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError { reason }),
            retry_from: anchor,
            semantic_trap_retry_override: None, .. } if reason.contains("Monthly enforcement unavailable") && *anchor == retry_from));
    assert!(
        crate::durable_host::ephemeral_suspend_trap(AgentMode::Ephemeral, &converted, retry_from)
            .is_none(),
        "the generic suspension conversion must preserve the monthly reason"
    );
    for (mode, trap, unavailable) in [
        (AgentMode::Durable, suspend.clone(), true),
        (AgentMode::Ephemeral, suspend, false),
        (
            AgentMode::Ephemeral,
            TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::from(51_u64))),
            true,
        ),
        (
            AgentMode::Ephemeral,
            TrapType::Interrupt(InterruptKind::ShardLost),
            true,
        ),
        (AgentMode::Ephemeral, TrapType::Exit, true),
    ] {
        assert!(
            monthly_failure::<crate::workerctx::default::Context>(
                &trap,
                mode,
                &None,
                &config,
                unavailable,
                retry_from,
            )
            .is_none()
        );
    }
}

struct WrappedOplogBootstrap(Arc<dyn crate::services::oplog::OplogService>);

#[async_trait::async_trait]
impl crate::Bootstrap<crate::workerctx::default::Context> for WrappedOplogBootstrap {
    fn create_additional_deps(
        &self,
        _: Arc<dyn golem_service_base::clients::registry::RegistryService>,
    ) -> crate::services::NoAdditionalDeps {
        crate::services::NoAdditionalDeps
    }

    fn wrap_oplog_service(
        &self,
        _: Arc<dyn crate::services::oplog::OplogService>,
    ) -> Arc<dyn crate::services::oplog::OplogService> {
        self.0.clone()
    }
}

/// A resident worker over a primary oplog whose indexed reads a test can fail, with the
/// production status actor and result lookup.
struct ResidentWorker {
    worker: Arc<Worker<crate::workerctx::default::Context>>,
    oplog: Arc<dyn Oplog>,
    indexed: Arc<crate::services::oplog::tests::ReadCountingIndexedStorage>,
    shutdown: crate::services::shutdown::Shutdown,
    _tasks: tokio::task::JoinSet<anyhow::Result<()>>,
}

impl ResidentWorker {
    async fn start() -> Self {
        use crate::services::HasShardService;
        use crate::services::golem_config::*;
        use crate::services::oplog::tests::{
            ReadCountingIndexedStorage, create_test_entry, default_execution_status,
            default_last_known_status, make_agent_metadata,
        };
        use crate::services::oplog::{OplogService, PrimaryOplogService};
        use crate::worker::status::calculate_last_known_status_for_existing_worker;
        use crate::worker::{GetOrCreateWorkerResult, ResolvedOwnerContext};
        use futures::FutureExt;
        use golem_common::model::account::{AccountEmail, AccountId};
        use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentKey, Snapshotting};
        use golem_common::model::application::{ApplicationId, ApplicationName};
        use golem_common::model::component::ComponentName;
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_common::model::diff::Hash;
        use golem_common::model::environment::{EnvironmentId, EnvironmentName};
        use golem_common::model::{AgentId, RetryConfig};
        use golem_common::schema::agent::{
            AgentConstructorSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
        };
        use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
        use golem_service_base::config::{BlobStorageConfig, InMemoryBlobStorageConfig};
        use golem_service_base::model::component::Component;
        use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
        use std::collections::{BTreeMap, HashMap};
        use std::time::Duration;

        let _ = rustls::crypto::ring::default_provider().install_default();
        let indexed = Arc::new(ReadCountingIndexedStorage::new());
        let service = Arc::new(
            PrimaryOplogService::new(
                indexed.clone(),
                Arc::new(InMemoryBlobStorage::new()),
                0,
                0,
                1024,
                0,
                RetryConfig {
                    max_attempts: 2,
                    min_delay: Duration::ZERO,
                    max_delay: Duration::ZERO,
                    ..Default::default()
                },
            )
            .await,
        );
        let mut config = GolemConfig {
            key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
            indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
            scheduler_storage: SchedulerStorageConfig::InMemory(SchedulerStorageInMemoryConfig {}),
            blob_storage: BlobStorageConfig::InMemory(InMemoryBlobStorageConfig {}),
            resource_limits: ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
            ..Default::default()
        };
        config.filesystem_storage.mode = FilesystemStorageMode::Temporary;
        config.engine.enable_fs_cache = false;
        config.oplog.sweep.enabled = false;
        config.agent_status_flush.enabled = false;
        config.agent_status_checkpoint.enabled = false;
        config.set_golem_issuer(crate::test_callers::TEST_ISSUER.parse().unwrap());
        let shutdown = crate::services::shutdown::Shutdown::new();
        let mut tasks = tokio::task::JoinSet::new();
        let (deps, _epoch, _) =
            crate::create_worker_executor_impl::<crate::workerctx::default::Context, _>(
                config,
                &WrappedOplogBootstrap(service.clone()),
                tokio::runtime::Handle::current(),
                &Arc::new(crate::services::worker_activator::LazyWorkerActivator::new()),
                shutdown.clone(),
                &mut tasks,
            )
            .await
            .unwrap();
        deps.shard_service().install_unexpiring(
            1,
            &HashMap::from([(
                golem_common::model::ShardId::new(0),
                golem_common::model::ShardEpoch(0),
            )]),
        );

        let id = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 1628));
        let account = AccountId::new();
        let parsed = ParsedAgentKey::new(
            AgentTypeName("Counter".into()),
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(Vec::new())),
                SchemaValue::Record { fields: Vec::new() },
            ),
        );
        let mut metadata = make_agent_metadata(id.agent_id, account, id.environment_id);
        metadata.agent_type_name = Some(parsed.agent_type.clone());
        metadata.constructor_parameters = Some(parsed.canonical_parameters().unwrap());
        let mut create = create_test_entry(
            id.agent_id,
            AgentMode::Durable,
            ComponentRevision::INITIAL,
            id.environment_id,
            account,
            metadata.fingerprint.0,
        );
        let OplogEntry::Create { parameters, .. } = &mut create else {
            unreachable!()
        };
        parameters.agent_type_name = metadata.agent_type_name.clone();
        parameters.constructor_parameters = metadata.constructor_parameters.clone();
        let oplog = service
            .create_fresh(
                &mut service.lock_lifecycle(&id.agent_id).await,
                &id,
                AgentMode::Durable,
                create,
                metadata.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
            .unwrap();
        let status =
            calculate_last_known_status_for_existing_worker(&deps, &id, AgentMode::Durable, None)
                .await
                .unwrap();
        let component = Component {
            id: metadata.component_id,
            revision: ComponentRevision::INITIAL,
            environment_id: id.environment_id,
            component_name: ComponentName("counter".into()),
            hash: Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: account,
            account_email: AccountEmail::new("owner@example.com"),
            application_name: ApplicationName("test".into()),
            environment_name: EnvironmentName::try_from("test").unwrap(),
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                Default::default(),
                Vec::new(),
                vec![AgentTypeSchema {
                    type_name: parsed.agent_type.clone(),
                    kind: AgentTypeKind::Regular,
                    description: String::new(),
                    source_language: String::new(),
                    schema: SchemaGraph::empty(),
                    constructor: AgentConstructorSchema {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: InputSchema::parameters([]),
                    },
                    methods: Vec::new(),
                    dependencies: Vec::new(),
                    mode: AgentMode::Durable,
                    http_mount: None,
                    snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
                    config: Vec::new(),
                }],
                BTreeMap::new(),
            ),
            created_at: chrono::Utc::now(),
            wasm_hash: Hash::empty(),
            package_metadata: None,
            object_store_key: String::new(),
        };
        metadata.last_known_status = status.clone();
        let initial = GetOrCreateWorkerResult {
            initial_worker_metadata: metadata,
            current_status: Arc::new(arc_swap::ArcSwap::from_pointee(status)),
            persisted_status: None,
            execution_status: Arc::new(std::sync::RwLock::new(
                crate::model::ExecutionStatus::Suspended {
                    agent_mode: AgentMode::Durable,
                    timestamp: Timestamp::now_utc(),
                },
            )),
            owner_context: ResolvedOwnerContext::Agent(Box::new(parsed)),
            snapshot_policy: SnapshotPolicy::Disabled,
            oplog: oplog.clone(),
            initial_component: Arc::new(component),
            reconstructed_ephemeral: false,
            newly_created: false,
        };
        let worker = Worker::<crate::workerctx::default::Context>::unresolved(
            &deps,
            Arc::new(crate::services::card_interest::CardInterestIndex::new()),
            id,
        );
        let constructing = worker.clone();
        let (_, initialized) = worker
            .initialize_with(
                async move {
                    constructing
                        .finish_construction(std::time::Instant::now(), Ok(initial))
                        .await
                }
                .boxed(),
            )
            .await;
        initialized.unwrap();
        Self {
            worker,
            oplog,
            indexed,
            shutdown,
            _tasks: tasks,
        }
    }

    async fn stop(self) {
        self.worker
            .state_actor
            .stop_handle()
            .stop_and_wait()
            .await
            .unwrap();
        self.worker.status_flusher.begin_delete().await;
        self.oplog.stop_and_wait().await.unwrap();
        self.shutdown
            .cancel_and_wait_for_tracked(std::time::Duration::from_secs(5))
            .await;
    }
}

/// An oplog that does not read for good stays as it is: the recovery failure of that read adds no
/// error entry, so nothing records the read error as an outcome of the agent. A recovery that
/// failed for another reason still records its error.
#[test]
#[test_r::timeout("60s")]
async fn a_recovery_failure_of_an_unreadable_oplog_adds_no_entry() {
    use crate::services::oplog::{OplogReadError, OplogReadSource, UnreadableOplog};

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let tail = resident.oplog.current_oplog_index().await;
    let unreadable = WorkerExecutorError::from(UnreadableOplog::new(
        &worker.agent_id(),
        OplogReadError::of_payload(
            OplogReadSource::Other("queued agent invocation request payload"),
            tail,
            crate::services::oplog::OplogPayloadDownloadError::Corrupt(anyhow::anyhow!(
                "unknown serialization version: 255"
            )),
        ),
    ));
    assert!(matches!(
        unreadable,
        WorkerExecutorError::OplogFailed { .. }
    ));
    for error in [
        unreadable.clone(),
        WorkerExecutorError::failed_to_resume_worker(worker.agent_id(), unreadable),
    ] {
        worker.record_recovery_failure(&error).await;
        assert_eq!(
            resident.oplog.current_oplog_index().await,
            tail,
            "{error} must not add an entry"
        );
    }

    worker
        .record_recovery_failure(&WorkerExecutorError::unknown("the guest stack overflowed"))
        .await;
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    resident.stop().await;
}

/// A start whose oplog read fails for a time gives a recovery that the read caused. The startup
/// records that recovery failure as the agent's start does, directly and wrapped in a failed
/// resume. Neither adds an entry; a recovery for another cause still adds its error entry.
#[test]
#[test_r::timeout("60s")]
async fn a_transient_oplog_read_at_startup_adds_no_recovery_entry() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let tail = resident.oplog.current_oplog_index().await;
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "startup read unavailable".into(),
        )));
    let reads = resident.indexed.reads();
    let unreadable = resident.oplog.read(OplogIndex::INITIAL).await.unwrap_err();
    assert!(
        resident.indexed.reads() > reads,
        "the read must reach storage"
    );
    let read_error = WorkerExecutorError::from(unreadable);
    assert!(
        matches!(&read_error, WorkerExecutorError::RecoveryRequired { details, .. }
            if details.contains("startup read unavailable")),
        "{read_error:?}"
    );
    resident.indexed.set_read_error(None);

    for error in [
        read_error.clone(),
        WorkerExecutorError::failed_to_resume_worker(worker.agent_id(), read_error),
    ] {
        worker.record_recovery_failure(&error).await;
        assert_eq!(
            resident.oplog.current_oplog_index().await,
            tail,
            "{error} must not add an entry"
        );
    }

    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required(
            "the component download did not answer",
        ))
        .await;
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    resident.stop().await;
}

/// A pending invocation is stored while the status fold cannot read, so the published status does
/// not know its key. A result lookup gives the read error of the fold again, instead of reading
/// the key as new; once reads pass, the fold knows the key and the lookup reads it as pending.
#[test]
#[test_r::timeout("60s")]
async fn a_result_lookup_on_a_detached_status_gives_the_read_error() {
    use crate::model::LookupResult;
    use crate::services::oplog::OplogOps;
    use crate::storage::indexed::IndexedStorageError;
    use golem_common::model::AgentInvocationPayload;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::schema::SchemaValue;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let key = InvocationId(uuid::Uuid::from_u128(0x1628_0003));
    let (operation, payload) = resident
        .oplog
        .upload_request(AgentInvocationPayload::AgentMethod {
            method_name: "count".into(),
            allow_streams: false,
            input: SchemaValue::Tuple { elements: vec![] }.try_into().unwrap(),
            caller: Caller::anonymous(),
            scope_card: None,
        })
        .await
        .unwrap();
    let context = InvocationContextStack::fresh_rounded();
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "pending fold read unavailable".into(),
        )));
    resident
        .oplog
        .add(OplogEntry::pending_agent_invocation(
            None,
            key,
            Box::new(operation),
            payload,
            context.trace_id.clone(),
            context.trace_states.clone(),
            context.to_oplog_data(),
        ))
        .await
        .unwrap();
    let commit = worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await;
    let published = worker.state_actor.status().await;
    assert!(
        !published
            .pending_invocations
            .iter()
            .any(|pending| pending.has_idempotency_key(&key))
            && worker.last_known_status_detached(),
        "the fold must fail: commit {commit:?}, reads {}, detached {}, pending {:?}",
        resident.indexed.reads(),
        worker.last_known_status_detached(),
        published.pending_invocations
    );

    let lookup = worker.lookup_invocation_result(&key).await;
    assert!(
        matches!(&lookup, Err(WorkerExecutorError::RecoveryRequired { details, .. })
            if details.contains("pending fold read unavailable")),
        "{lookup:?}"
    );
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Other(
            "pending fold entry corrupt".into(),
        )));
    let lookup = worker.try_lookup_invocation_result(&key).await;
    assert!(
        matches!(&lookup, Err(WorkerExecutorError::OplogFailed { details, .. })
            if details.contains("pending fold entry corrupt")),
        "{lookup:?}"
    );

    resident.indexed.set_read_error(None);
    assert_eq!(
        worker.try_lookup_invocation_result(&key).await.unwrap(),
        LookupResult::Pending
    );
    assert_eq!(
        worker.lookup_invocation_result(&key).await.unwrap(),
        LookupResult::Pending
    );
    resident.stop().await;
}
