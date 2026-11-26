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

/// A persisted result index, or the background failure index of a result, that names a
/// readable entry of another kind is a permanently unreadable oplog of the agent at that index.
/// The result stays lazy, so nothing takes the read as an outcome.
#[test]
async fn a_result_index_at_an_entry_of_another_kind_is_unreadable_at_that_index() {
    let invocation = AgentInvocationResult::AgentMethod {
        output: golem_common::schema::SchemaValue::U32(42),
    };
    let (services, _, finished, id, fingerprint) = lazy_result(OplogPayload::SerializedInline {
        bytes: golem_common::serialization::serialize(&invocation).unwrap(),
        cached: None,
    })
    .await;
    let suspend = OplogIndex::from_u64(finished.as_u64() - 1);
    let cases = [
        (suspend, None, "invocation result index"),
        (finished, Some(suspend), "background failure index"),
    ];
    let outcomes = futures::future::join_all(cases.into_iter().map(
        |(oplog_idx, background_failure, source)| {
            let services = &services;
            let id = &id;
            async move {
                let mut result = InvocationResult::Lazy {
                    oplog_idx,
                    background_failure,
                };
                let error = result
                    .cache(id, AgentMode::Durable, fingerprint, services)
                    .await
                    .unwrap_err();
                (result, oplog_idx, background_failure, source, error)
            }
        },
    ))
    .await;
    assert!(
        outcomes
            .into_iter()
            .all(|(result, oplog_idx, background_failure, source, error)| {
                let message = error.to_string();
                error.agent_id == id.agent_id
                    && matches!(
                        &error.error,
                        OplogReadError::Corruption {
                            source: crate::services::oplog::OplogReadSource::Other(named),
                            ..
                        } if *named == source
                    )
                    && message.contains(&format!("oplog index {suspend}"))
                    && matches!(
                        WorkerExecutorError::from(error),
                        WorkerExecutorError::OplogFailed { .. }
                    )
                    && matches!(
                        result,
                        InvocationResult::Lazy { oplog_idx: idx, background_failure: failure }
                            if idx == oplog_idx && failure == background_failure
                    )
            })
    );
}

#[test]
fn invocation_classification_uses_the_recovery_cause_and_the_resume_reason() {
    let agent_id = golem_common::model::AgentId::from_parts(7, 1628);
    let failed = WorkerExecutorError::oplog_failed(agent_id, "entry 4 has version 254");
    let unavailable = WorkerExecutorError::oplog_read_unavailable("primary did not answer");
    let other = WorkerExecutorError::recovery_required("stream output 3 is incomplete");
    // An archive layer that did not store the oplog stops the agent, but it is not a read.
    let archive_write =
        WorkerExecutorError::from(crate::services::oplog::OplogError::ArchiveWrite(
            crate::services::oplog::ArchiveWriteFailure {
                agent_id,
                level: 1,
                details: "archive layer 1 refused the transfer".to_string(),
            },
        ));
    let wrap = |error: &WorkerExecutorError| {
        WorkerExecutorError::failed_to_resume_worker(agent_id, error.clone())
    };
    let cases = [
        (
            archive_write.clone(),
            Some(InvocationObservationFailure::ArchiveWrite),
            false,
        ),
        (
            wrap(&archive_write),
            Some(InvocationObservationFailure::ArchiveWrite),
            false,
        ),
        (
            failed.clone(),
            Some(InvocationObservationFailure::PermanentRead),
            true,
        ),
        (
            wrap(&failed),
            Some(InvocationObservationFailure::PermanentRead),
            true,
        ),
        (
            unavailable.clone(),
            Some(InvocationObservationFailure::RetryableRead),
            true,
        ),
        (
            wrap(&wrap(&unavailable)),
            Some(InvocationObservationFailure::RetryableRead),
            true,
        ),
        (
            other.clone(),
            Some(InvocationObservationFailure::Recovery),
            false,
        ),
        (wrap(&other), None, false),
        (
            WorkerExecutorError::ShardingNotReady,
            Some(InvocationObservationFailure::Reroute),
            false,
        ),
        (WorkerExecutorError::unknown("guest trapped"), None, false),
    ];
    let wrong = cases
        .into_iter()
        .filter(|(error, kind, read)| {
            InvocationObservationFailure::classify(error) != *kind
                || InvocationObservationFailure::is_stored_read(error) != *read
        })
        .map(|(error, _, _)| error)
        .collect::<Vec<_>>();
    assert!(wrong.is_empty(), "{wrong:?}");
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
        Self::start_on(None).await
    }

    /// A resident worker whose primary oplog writes to `unconfirmed`, when it is given, so that a
    /// test can make an append whose outcome is unknown.
    async fn start_on(
        unconfirmed: Option<Arc<crate::services::oplog::UnconfirmedAppends>>,
    ) -> Self {
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
        let oplog_storage: Arc<dyn crate::storage::indexed::IndexedStorage + Send + Sync> =
            match unconfirmed {
                Some(unconfirmed) => unconfirmed,
                None => indexed.clone(),
            };
        let service = Arc::new(
            PrimaryOplogService::new(
                oplog_storage,
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

    /// Installs a running generation whose commands the test receives, without an instance.
    async fn install_running(&self) -> tokio::sync::mpsc::UnboundedReceiver<WorkerCommand> {
        use crate::worker::lifecycle::state::LifecycleTransition;
        use crate::worker::{RunningWorker, WaitingWorker};
        let (sender, commands) = tokio::sync::mpsc::unbounded_channel();
        let start_attempt = uuid::Uuid::new_v4();
        let mut lifecycle = self.worker.instance.lock().await;
        let LifecycleTransition::Unloaded(unloaded) = lifecycle.transition() else {
            panic!("a resident worker starts unloaded");
        };
        unloaded.wait_for_permit(WaitingWorker {
            handle: tokio::spawn(async {}),
            start_attempt,
        });
        let LifecycleTransition::WaitingForPermit(waiting) = lifecycle.transition() else {
            unreachable!()
        };
        waiting.install_running(RunningWorker {
            handle: tokio::spawn(async {}),
            sender,
            queue: Default::default(),
            waiting_for_command: Default::default(),
            concurrent_agent_permit_held: Default::default(),
            filesystem_activity: Default::default(),
            filesystem_snapshot_slot: Default::default(),
            unload_request: Default::default(),
            idle_since_millis: Default::default(),
            interrupts: Default::default(),
            start_attempt,
            owner_generation_started: false,
        });
        commands
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
    let errors = [
        unreadable.clone(),
        WorkerExecutorError::failed_to_resume_worker(worker.agent_id(), unreadable),
    ];
    futures::future::join_all(
        errors
            .iter()
            .map(|error| worker.record_recovery_failure(error)),
    )
    .await;
    assert_eq!(
        resident.oplog.current_oplog_index().await,
        tail,
        "{errors:?} must not add an entry"
    );

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

    let errors = [
        read_error.clone(),
        WorkerExecutorError::failed_to_resume_worker(worker.agent_id(), read_error),
    ];
    futures::future::join_all(
        errors
            .iter()
            .map(|error| worker.record_recovery_failure(error)),
    )
    .await;
    assert_eq!(
        resident.oplog.current_oplog_index().await,
        tail,
        "{errors:?} must not add an entry"
    );

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

/// A stop for a start whose oplog read failed fails the pending invocations with that error,
/// wrapped in a failed resume. The read error is not an outcome: the stop caches no result for a
/// pending invocation, a later lookup reads it as pending, and its waiter gets the read error.
#[test]
#[test_r::timeout("60s")]
async fn a_pending_invocation_failed_with_a_wrapped_read_error_gets_no_cached_result() {
    use crate::model::LookupResult;
    use crate::services::oplog::{OplogOps, OplogReadSource, UnreadableOplog};
    use golem_common::model::AgentInvocationPayload;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::schema::SchemaValue;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let key = InvocationId(uuid::Uuid::from_u128(0x1628_0b01));
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
    worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await
        .unwrap();
    assert_eq!(
        worker.try_lookup_invocation_result(&key).await.unwrap(),
        LookupResult::Pending
    );

    let unreadable = WorkerExecutorError::from(UnreadableOplog::new(
        &worker.agent_id(),
        OplogReadError::corruption(
            OplogReadSource::Primary,
            "failed to decode oplog entry 3: unknown serialization version: 254",
        ),
    ));
    let wrapped = WorkerExecutorError::failed_to_resume_worker(worker.agent_id(), unreadable);
    let subscription = worker.events().subscribe();
    worker.fail_pending_invocations(wrapped.clone()).await;

    assert_eq!(
        worker.try_lookup_invocation_result(&key).await.unwrap(),
        LookupResult::Pending,
        "the stop must cache no result for the read error"
    );
    let observed = worker.wait_for_invocation_result(&key, subscription).await;
    assert_eq!(observed, Err(wrapped));
    resident.stop().await;
}

/// An automatic update whose commit stored its `PendingUpdate` but whose status after the commit
/// did not read fails with the read error. The caller's retry repairs the status before the
/// duplicate check, finds the stored update, and appends nothing more.
#[test]
#[test_r::timeout("60s")]
async fn a_retried_automatic_update_after_a_failed_report_appends_no_second_update() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let tail = resident.oplog.current_oplog_index().await;
    let target = ComponentRevision::new(2).unwrap();
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "update report unavailable".into(),
        )));
    let first = worker
        .enqueue_automatic_update(target, Caller::anonymous())
        .await;
    resident.indexed.set_read_error(None);
    assert!(
        matches!(&first, Err(error) if InvocationObservationFailure::is_stored_read(error)),
        "{first:?}"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());

    let retry = worker
        .enqueue_automatic_update(target, Caller::anonymous())
        .await;
    assert!(
        matches!(&retry, Err(WorkerExecutorError::InvalidRequest { details })
            if details.contains("already in progress")),
        "{retry:?}"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    resident.stop().await;
}

/// A keyless manual update whose commit stored its entry but whose status after the commit did
/// not read fails with the read error, not as a write that did not happen. The stored update
/// wakes the running loop. The repaired status holds the stored update, which the duplicate check
/// of a retried update reads.
#[test]
#[test_r::timeout("60s")]
async fn a_manual_update_after_a_failed_report_is_stored_and_found_by_the_repaired_status() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    let mut commands = resident.install_running().await;
    let tail = resident.oplog.current_oplog_index().await;
    let target = ComponentRevision::new(3).unwrap();
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "manual update report unavailable".into(),
        )));
    let first = worker
        .enqueue_manual_update(target, Caller::anonymous())
        .await;
    resident.indexed.set_read_error(None);
    assert!(
        matches!(&first, Err(error) if InvocationObservationFailure::is_stored_read(error)),
        "{first:?}"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    let woken = std::iter::from_fn(|| commands.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        woken
            .iter()
            .any(|command| matches!(command, WorkerCommand::WorkAvailable)),
        "the stored update must wake the loop, got {} commands",
        woken.len()
    );
    let status = worker.try_get_repaired_status().await.unwrap();
    assert!(
        status
            .pending_invocations
            .iter()
            .any(|pending| pending.manual_update_target_revision == Some(target)),
        "{:?}",
        status.pending_invocations
    );
    resident.stop().await;
}

/// An append whose outcome is unknown is `Unconfirmed` for a lifecycle write, not `Unreadable`,
/// so the caller cannot take it as a stored entry, and the write sends no wakeup for it. The
/// status actor wakes the owner, because the handle refuses every later write.
#[test]
#[test_r::timeout("60s")]
async fn an_unconfirmed_lifecycle_write_is_not_reported_as_stored() {
    use crate::services::oplog::{OplogError, UnconfirmedAppends};
    use crate::storage::indexed::IndexedStorageError;

    let storage = Arc::new(UnconfirmedAppends::default());
    let resident = ResidentWorker::start_on(Some(storage.clone())).await;
    let worker = &resident.worker;
    let mut commands = resident.install_running().await;
    storage.unconfirm(IndexedStorageError::Transient(
        "read-back outage".to_string(),
    ));
    let guard = worker.lock_non_stopping_worker().await;
    let written = worker
        .add_and_commit_oplog_internal(
            &guard,
            OplogEntry::grow_memory(5),
            Some(WorkerCommand::WorkAvailable),
        )
        .await;
    drop(guard);

    let unconfirmed = resident.oplog.unconfirmed_append().expect("latched");
    assert_eq!(written, Err(OplogError::Unconfirmed(unconfirmed)));
    let woken = tokio::time::timeout(std::time::Duration::from_secs(10), commands.recv())
        .await
        .expect("the owner is woken")
        .expect("the channel stays open");
    assert!(
        matches!(woken, WorkerCommand::InternalStatusChanged),
        "a write with an unknown outcome sends no wakeup of its own"
    );
    resident.stop().await;
}

/// A background flush that latches an append whose outcome is unknown wakes the owner, which
/// waits for work and makes no write of its own, so that its generation stops.
#[test]
#[test_r::timeout("60s")]
async fn a_background_flush_with_an_unknown_outcome_wakes_the_owner() {
    use crate::services::oplog::{OplogError, UnconfirmedAppends};
    use crate::storage::indexed::IndexedStorageError;

    let storage = Arc::new(UnconfirmedAppends::default());
    let resident = ResidentWorker::start_on(Some(storage.clone())).await;
    let mut commands = resident.install_running().await;
    storage.unconfirm(IndexedStorageError::Transient(
        "read-back outage".to_string(),
    ));

    let flushed = resident.oplog.add(OplogEntry::grow_memory(6)).await;

    assert!(
        matches!(flushed, Err(OplogError::Unconfirmed(_))),
        "{flushed:?}"
    );
    let woken = tokio::time::timeout(std::time::Duration::from_secs(10), commands.recv())
        .await
        .expect("the owner is woken")
        .expect("the channel stays open");
    assert!(matches!(woken, WorkerCommand::InternalStatusChanged));
    resident.stop().await;
}

/// A generation that starts on a handle with an unconfirmed append stops before it creates an
/// instance. The waiters of the start get the read error of the append.
#[test]
#[test_r::timeout("60s")]
async fn a_start_on_a_handle_with_an_unconfirmed_append_stops_with_its_read_error() {
    use crate::services::HasEvents;
    use crate::services::events::Event;
    use crate::services::oplog::UnconfirmedAppends;
    use crate::storage::indexed::IndexedStorageError;

    let storage = Arc::new(UnconfirmedAppends::default());
    let resident = ResidentWorker::start_on(Some(storage.clone())).await;
    let worker = &resident.worker;
    storage.unconfirm(IndexedStorageError::Transient(
        "read-back outage".to_string(),
    ));
    let _ = resident.oplog.add(OplogEntry::grow_memory(7)).await;
    let unconfirmed = resident.oplog.unconfirmed_append().expect("latched");
    let agent_id = worker.agent_id();
    let mut events = worker.events().subscribe();

    Worker::start_if_needed(worker.clone()).await.unwrap();
    let loaded = events
        .wait_for(|event| match event {
            Event::WorkerLoaded {
                agent_id: loaded,
                result,
                ..
            } if *loaded == agent_id => Some(result.clone()),
            _ => None,
        })
        .await
        .unwrap();

    assert_eq!(loaded, Err(WorkerExecutorError::from(unconfirmed)));
    resident.stop().await;
}

/// A recovery failure that is not a read and that names its retry point adds its error entry,
/// also when the error history does not read: the history is read only for a retry point. A
/// recovery failure without a retry point needs the history, so it adds no entry while the
/// history does not read.
#[test]
#[test_r::timeout("60s")]
async fn a_recovery_failure_reads_the_history_only_without_a_retry_point() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required(
            "the component download did not answer",
        ))
        .await;
    let tail = resident.oplog.current_oplog_index().await;
    assert_eq!(
        worker.get_last_known_status().await.last_error_kind,
        Some(OplogErrorKind::Recovery)
    );
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "error history unavailable".into(),
        )));
    let reads = resident.indexed.reads();

    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required(
            "the component download did not answer again",
        ))
        .await;
    assert!(
        resident.indexed.reads() > reads,
        "the history read must reach storage"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail);

    let retry_point = OplogIndex::from_u64(1);
    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required_from(
            "the component download failed at a known point",
            retry_point,
        ))
        .await;
    resident.indexed.set_read_error(None);
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    let recorded = resident.oplog.read(tail.next()).await.unwrap();
    assert!(
        matches!(&recorded, OplogEntry::Error { retry_from, .. } if *retry_from == retry_point),
        "{recorded:?}"
    );
    resident.stop().await;
}

/// A failed agent whose error history does not read refuses new work with the read error, not
/// with a made-up prior failure.
#[test]
#[test_r::timeout("60s")]
async fn a_failed_agent_with_an_unreadable_history_refuses_with_the_read_error() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    worker
        .record_recovery_failure(&WorkerExecutorError::invalid_request(
            "the constructor parameters do not decode",
        ))
        .await;
    let status = worker.get_last_known_status().await;
    assert_eq!(status.status, AgentStatus::Failed);
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "error history unavailable".into(),
        )));

    let refused = Worker::<crate::workerctx::default::Context>::ensure_not_failed(
        &worker.deps,
        &worker.owned_agent_id,
        worker.agent_mode(),
        &status,
    )
    .await;
    resident.indexed.set_read_error(None);

    assert!(
        matches!(&refused, Err(WorkerExecutorError::RecoveryRequired { details, .. })
            if details.contains("error history unavailable")),
        "{refused:?}"
    );
    resident.stop().await;
}
