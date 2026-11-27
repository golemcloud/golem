use super::*;
use crate::durable_host::durable_stream::tests::TestOplog;
use crate::services::oplog::{Oplog, OplogReadError};
use crate::worker::interrupt::Interrupts;
use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt, UnloadReason, UnloadRequest};
use futures::StreamExt;
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

/// A result index at an `Exited` entry caches the exit as the result, and a background failure
/// index at an `Exited` entry is the exit of a finished result. Neither is a read error.
#[test]
async fn a_result_or_a_background_failure_at_an_exit_caches_the_exit() {
    let invocation = AgentInvocationResult::AgentMethod {
        output: golem_common::schema::SchemaValue::U32(42),
    };
    let (services, _, finished, id, fingerprint) = lazy_result(OplogPayload::SerializedInline {
        bytes: golem_common::serialization::serialize(&invocation).unwrap(),
        cached: None,
    })
    .await;
    let exited = services
        .0
        .add(OplogEntry::Exited {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();

    let mut exit = InvocationResult::Lazy {
        oplog_idx: exited,
        background_failure: None,
    };
    exit.cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap();
    assert!(
        matches!(
            &exit,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult::Trap {
                    trap_type: TrapType::Exit,
                    ..
                })
            }
        ),
        "{exit:?}"
    );

    let mut background = InvocationResult::Lazy {
        oplog_idx: finished,
        background_failure: Some(exited),
    };
    background
        .cache(&id, AgentMode::Durable, fingerprint, &services)
        .await
        .unwrap();
    assert!(
        matches!(
            &background,
            InvocationResult::Cached { result: Ok(output) }
                if output.result == invocation
                    && output.background_failure == Some(BackgroundFailure::Exited)
        ),
        "{background:?}"
    );
}

#[test]
fn invocation_classification_uses_the_recovery_cause_and_the_resume_reason() {
    let agent_id = golem_common::model::AgentId::from_parts(7, 1628);
    let failed = WorkerExecutorError::oplog_failed(agent_id, "entry 4 has version 254");
    let unavailable = WorkerExecutorError::oplog_read_unavailable("primary did not answer");
    let other = WorkerExecutorError::recovery_required("stream output 3 is incomplete");
    let wrap = |error: &WorkerExecutorError| {
        WorkerExecutorError::failed_to_resume_worker(agent_id, error.clone())
    };
    // A read that failed for good fails the agent: it is not an observation failure.
    let cases = [
        (failed.clone(), None, false),
        (wrap(&failed), None, false),
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

struct WrappedOplogBootstrap(
    Arc<dyn crate::services::oplog::OplogService>,
    crate::storage::keyvalue::fault_injecting::KeyValueStorageFaults,
);

#[async_trait::async_trait]
impl crate::Bootstrap<crate::workerctx::default::Context> for WrappedOplogBootstrap {
    fn wrap_key_value_storage(
        &self,
        storage: Arc<dyn crate::storage::keyvalue::KeyValueStorage + Send + Sync>,
    ) -> Arc<dyn crate::storage::keyvalue::KeyValueStorage + Send + Sync> {
        Arc::new(
            crate::storage::keyvalue::fault_injecting::FaultInjectingKeyValueStorage::new(
                storage,
                self.1.clone(),
            ),
        )
    }

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

/// A resident worker over a primary oplog whose indexed reads a test can fail, and over a
/// key-value storage whose labelled operations a test can fail and count, with the production
/// status actor and result lookup.
struct ResidentWorker {
    worker: Arc<Worker<crate::workerctx::default::Context>>,
    oplog: Arc<dyn Oplog>,
    indexed: Arc<crate::services::oplog::tests::ReadCountingIndexedStorage>,
    kv_faults: crate::storage::keyvalue::fault_injecting::KeyValueStorageFaults,
    shutdown: crate::services::shutdown::Shutdown,
    _tasks: tokio::task::JoinSet<anyhow::Result<()>>,
}

impl ResidentWorker {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// A resident worker of an executor whose configuration `configure` changes.
    async fn start_with(configure: impl FnOnce(&mut GolemConfig)) -> Self {
        Self::start_owner(configure, None).await
    }

    /// A resident standalone tool owner of the invocation `key`.
    async fn start_standalone(key: InvocationId) -> Self {
        Self::start_owner(|_| {}, Some(key)).await
    }

    async fn start_owner(
        configure: impl FnOnce(&mut GolemConfig),
        standalone: Option<InvocationId>,
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
        let kv_faults = crate::storage::keyvalue::fault_injecting::KeyValueStorageFaults::default();
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
        configure(&mut config);
        config.set_golem_issuer(crate::test_callers::TEST_ISSUER.parse().unwrap());
        let shutdown = crate::services::shutdown::Shutdown::new();
        let mut tasks = tokio::task::JoinSet::new();
        let (deps, _epoch, _) =
            crate::create_worker_executor_impl::<crate::workerctx::default::Context, _>(
                config,
                &WrappedOplogBootstrap(service.clone(), kv_faults.clone()),
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
        if let Some(invocation_id) = standalone {
            let tool = Box::new(golem_common::model::oplog::StandaloneToolInvocation {
                tool_name: "standalone-owner".to_string().try_into().unwrap(),
                command_path: vec![],
                invocation_id,
                caller: Caller::anonymous(),
            });
            parameters.owner_kind = golem_common::model::agent::OwnerKind::StandaloneTool;
            parameters.standalone_tool = Some(tool.clone());
            metadata.owner_kind = golem_common::model::agent::OwnerKind::StandaloneTool;
            metadata.standalone_tool = Some(tool);
        }
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
                BTreeMap::from([(
                    parsed.agent_type.clone(),
                    golem_common::base_model::component_metadata::AgentTypeProvisionConfig {
                        initial_permissions:
                            golem_common::model::component::AgentTypeInitialPermissions::default_for_recipient(
                                golem_common::model::card::recipient::RecipientPattern::Account {
                                    account: AccountEmail::new("owner@example.com"),
                                },
                            )
                            .to_polymorphic_card(),
                        plugins: Default::default(),
                        env: BTreeMap::new(),
                        config: Vec::new(),
                        files: Vec::new(),
                    },
                )]),
            ),
            created_at: jiff::Timestamp::now(),
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
            kv_faults,
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

/// An oplog that does not read for good fails its agent: the recovery failure of that read adds its
/// error entry, also when a failed resume carries it, as any other recovery failure does. Only a
/// read that failed for a time adds no entry.
#[test]
#[test_r::timeout("60s")]
async fn a_recovery_failure_of_a_permanently_unreadable_oplog_adds_its_error_entry() {
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
    worker.record_recovery_failure(&unreadable).await;
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    worker
        .record_recovery_failure(&WorkerExecutorError::failed_to_resume_worker(
            worker.agent_id(),
            unreadable,
        ))
        .await;
    assert_eq!(
        resident.oplog.current_oplog_index().await,
        tail.next().next()
    );
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

/// A stop for a start whose oplog read failed for a time fails the pending invocations with that
/// error, wrapped in a failed resume. The read error is not an outcome: the stop caches no result
/// for a pending invocation, a later lookup reads it as pending, and its waiter gets the read
/// error.
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
        OplogReadError::source_unavailable(OplogReadSource::Primary, "primary read timed out"),
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

/// A recovery failure without a retry point is recorded at the current index when the error
/// history fails for good: the agent fails, and no read waits for a history that never reads.
#[test]
#[test_r::timeout("60s")]
async fn a_recovery_failure_over_a_history_that_fails_for_good_is_recorded_at_the_current_index() {
    use crate::storage::indexed::IndexedStorageError;

    let resident = ResidentWorker::start().await;
    let worker = &resident.worker;
    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required(
            "the component download did not answer",
        ))
        .await;
    let tail = resident.oplog.current_oplog_index().await;
    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Other(
            "error history entry has version 254".into(),
        )));
    let reads = resident.indexed.reads();

    worker
        .record_recovery_failure(&WorkerExecutorError::recovery_required(
            "the component download did not answer again",
        ))
        .await;
    let history_read = resident.indexed.reads() > reads;
    resident.indexed.set_read_error(None);

    assert!(history_read, "the history read must reach storage");
    assert_eq!(resident.oplog.current_oplog_index().await, tail.next());
    let recorded = resident.oplog.read(tail.next()).await.unwrap();
    assert!(
        matches!(&recorded, OplogEntry::Error { retry_from, .. } if *retry_from == tail),
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

/// A terminal standalone owner whose own invocation is still pending reports a read of its error
/// history that failed for a time as the read error of the lookup, not as the completed result of
/// that invocation.
#[test]
#[test_r::timeout("60s")]
async fn a_terminal_standalone_owner_with_an_unanswered_error_history_has_no_completed_result() {
    use crate::model::LookupResult;
    use crate::services::oplog::OplogOps;
    use crate::storage::indexed::IndexedStorageError;
    use golem_common::model::AgentInvocationPayload;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::schema::SchemaValue;

    let key = InvocationId(uuid::Uuid::from_u128(0x1628_00f5));
    let resident = ResidentWorker::start_standalone(key).await;
    let worker = &resident.worker;
    let (operation, payload) = resident
        .oplog
        .upload_request(AgentInvocationPayload::AgentMethod {
            method_name: "run".into(),
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
    worker
        .record_recovery_failure(&WorkerExecutorError::invalid_request(
            "the standalone owner does not start",
        ))
        .await;
    let status = worker.get_last_known_status().await;
    assert_eq!(status.status, AgentStatus::Failed);
    assert!(
        status
            .pending_invocations
            .iter()
            .any(|pending| pending.has_idempotency_key(&key)),
        "the owner's invocation stays pending"
    );

    resident
        .indexed
        .set_read_error(Some(IndexedStorageError::Transient(
            "error history unavailable".into(),
        )));
    let unanswered = worker.try_lookup_invocation_result(&key).await;
    resident.indexed.set_read_error(None);
    let answered = worker.try_lookup_invocation_result(&key).await;

    assert!(
        matches!(&unanswered, Err(WorkerExecutorError::RecoveryRequired { details, .. })
            if details.contains("error history unavailable")),
        "a read that failed for a time is not a completed result: {unanswered:?}"
    );
    assert!(
        matches!(&answered, Ok(LookupResult::Complete(Err(_)))),
        "the readable error history completes the owner's invocation: {answered:?}"
    );
    resident.stop().await;
}

/// A resident worker with one recent result whose index records a withdrawal of `key` over an
/// older entry of the key: a success when `older_success`, else a cancellation. The older entry
/// leaves the recent results before the withdrawal, so the index is not exact-complete. Returns
/// the worker, the older entry and the withdrawal.
async fn evicted_withdrawal(
    key: InvocationId,
    older_success: bool,
) -> (ResidentWorker, OplogIndex, OplogIndex) {
    use golem_common::model::InvocationEnd;

    let resident = ResidentWorker::start_with(|config| {
        config.invocation_results.recent_capacity = 1;
        config.invocation_results.bloom_bits = 128;
        config.invocation_results.bloom_hashes = 3;
    })
    .await;
    let older = resident
        .oplog
        .add(if older_success {
            OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                invocation_id: key,
                result: OplogPayload::SerializedInline {
                    bytes: golem_common::serialization::serialize(&cached_output(42).result)
                        .unwrap(),
                    cached: None,
                },
                method_name: Some("count".into()),
                consumed_fuel: 0,
                component_revision: ComponentRevision::INITIAL,
                awaits_settlement: false,
            }
        } else {
            OplogEntry::CancelPendingInvocation {
                timestamp: Timestamp::now_utc(),
                invocation_id: key,
            }
        })
        .await
        .unwrap();
    futures::stream::iter(1..=3u128)
        .for_each(|n| {
            let oplog = resident.oplog.clone();
            async move {
                oplog
                    .add(OplogEntry::CancelPendingInvocation {
                        timestamp: Timestamp::now_utc(),
                        invocation_id: InvocationId(uuid::Uuid::from_u128(n)),
                    })
                    .await
                    .unwrap();
            }
        })
        .await;
    let withdrawal = resident
        .oplog
        .add(OplogEntry::CancelPendingInvocation {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
        })
        .await
        .unwrap();
    resident
        .worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await
        .unwrap();
    let status = resident.worker.try_get_repaired_status().await.unwrap();
    assert!(!status.invocation_results.is_exact_complete());
    assert_eq!(
        status
            .invocation_results
            .position(&key)
            .map(|position| (position.terminal, position.end)),
        Some((withdrawal, InvocationEnd::Cancelled)),
        "the fold records the withdrawal of the evicted key"
    );
    (resident, older, withdrawal)
}

/// The invocation loop withdraws a queued duplicate of a key that already has a result. With one
/// recent result, the result of the key leaves the recent results before the withdrawal, so the
/// fold records the withdrawal and the lookup reads the older result from the result index. An
/// older success outranks the withdrawal. An older cancellation does not: the withdrawal stays the
/// answer, at its own entry.
#[test]
#[test_r::timeout("60s")]
async fn a_withdrawal_over_an_evicted_result_gives_way_only_to_an_older_success() {
    use crate::model::LookupResult;

    let key = InvocationId(uuid::Uuid::from_u128(0x1628_0303));
    let cases = futures::stream::iter([true, false])
        .then(|older_success| async move {
            let (resident, older, withdrawal) = evicted_withdrawal(key, older_success).await;
            let worker = &resident.worker;
            let status = worker.try_get_repaired_status().await.unwrap();
            let looked_up = worker.try_lookup_invocation_result(&key).await.unwrap();
            let cached_at = worker
                .hydrated_invocation_results
                .read()
                .await
                .get_valid(&key, &status)
                .map(|(_, index)| index);
            resident.stop().await;
            (older_success, older, withdrawal, looked_up, cached_at)
        })
        .collect::<Vec<_>>()
        .await;

    cases
        .into_iter()
        .for_each(|(older_success, older, withdrawal, looked_up, cached_at)| {
            if older_success {
                assert!(
                    matches!(&looked_up, LookupResult::Complete(Ok(output))
                        if output.result == cached_output(42).result),
                    "an older success outranks the withdrawal: {looked_up:?}"
                );
                assert_eq!(cached_at, Some(older));
            } else {
                assert!(
                    matches!(
                        &looked_up,
                        LookupResult::Complete(Err(WorkerExecutorError::InvocationCancelled))
                    ),
                    "an older cancellation leaves the withdrawal the answer: {looked_up:?}"
                );
                assert_eq!(
                    cached_at,
                    Some(withdrawal),
                    "the answer is the withdrawal at its own entry, not the older cancellation"
                );
            }
        });
}

/// Only an index that is not exact-complete reads the older result of a withdrawn key from the
/// persisted result index. When that index read fails, a lookup of the withdrawal over an evicted
/// success fails with the typed lookup error: it is not the cancellation, a miss or the older
/// success. A cancellation in an exact-complete index is the answer without an index read.
#[test]
#[test_r::timeout("60s")]
async fn a_lookup_reads_the_older_result_of_a_withdrawal_only_while_the_index_is_incomplete() {
    use crate::model::LookupResult;
    use crate::storage::keyvalue::KeyValueStorageError;

    const INDEX_READ: &str = "lookup_invocation_result_index";
    let key = InvocationId(uuid::Uuid::from_u128(0x1628_0305));

    let (incomplete, _, withdrawal) = evicted_withdrawal(key, true).await;
    incomplete.kv_faults.fail(
        INDEX_READ,
        2,
        KeyValueStorageError::Other("result index outage".into()),
    );
    let failed = incomplete.worker.try_lookup_invocation_result(&key).await;
    assert!(
        matches!(&failed, Err(error) if InvocationObservationFailure::is_stored_read(error)
            && format!("{error:?}").contains("invocation result index")),
        "the failed read of the result index is the error of the lookup: {failed:?}"
    );
    assert_eq!(incomplete.oplog.current_oplog_index().await, withdrawal);
    incomplete.stop().await;

    let complete = ResidentWorker::start().await;
    let cancelled = complete
        .oplog
        .add(OplogEntry::CancelPendingInvocation {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
        })
        .await
        .unwrap();
    complete
        .worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await
        .unwrap();
    let status = complete.worker.try_get_repaired_status().await.unwrap();
    assert!(status.invocation_results.is_exact_complete());
    let index_reads = complete.kv_faults.calls(INDEX_READ);
    complete.kv_faults.fail(
        INDEX_READ,
        2,
        KeyValueStorageError::Other("result index outage".into()),
    );
    let answered = complete.worker.try_lookup_invocation_result(&key).await;
    assert!(
        matches!(
            &answered,
            Ok(LookupResult::Complete(Err(
                WorkerExecutorError::InvocationCancelled
            )))
        ),
        "an exact-complete index answers with the cancellation: {answered:?}"
    );
    assert_eq!(
        complete.kv_faults.calls(INDEX_READ),
        index_reads,
        "an exact-complete index does not read the persisted result index"
    );
    assert_eq!(complete.oplog.current_oplog_index().await, cancelled);
    complete.stop().await;
}

/// An output of a method, as a result cache holds it.
fn cached_output(value: u32) -> AgentInvocationOutput {
    AgentInvocationOutput {
        result: AgentInvocationResult::AgentMethod {
            output: golem_common::schema::SchemaValue::U32(value),
        },
        consumed_fuel: None,
        invocation_status: None,
        component_revision: None,
        agent_id: None,
        idempotency_key: None,
        oplog_index: None,
        agent_fingerprint: None,
        background_failure: None,
    }
}

/// Waits for `operation` for at most five seconds, so that a wait that never ends fails the test
/// with `what` instead of a timeout of the test.
async fn within_five_seconds<T>(what: &str, operation: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .unwrap_or_else(|_| panic!("{what}"))
}

/// A wait for the initialization of a running agent ends with the notification of its waiters
/// when no recorded result proves the outcome. A wait that the agent interrupted before it
/// recorded a result ends as interrupted, and a wait whose notification is a read error that
/// failed for a time ends with that error. Neither waits again, and neither is a constructor
/// success, also when the agent records a success after the interrupted wait ended.
#[test]
#[test_r::timeout("60s")]
async fn an_initialization_wait_without_a_recorded_result_ends_with_its_notification() {
    use crate::services::events::Event;

    let resident = ResidentWorker::start().await;
    let worker = resident.worker.clone();
    let _commands = resident.install_running().await;
    let key = InvocationId::constructor(worker.initial_worker_metadata.fingerprint);
    let tail = resident.oplog.current_oplog_index().await;
    let generation = worker
        .try_get_repaired_status()
        .await
        .unwrap()
        .invocation_results
        .revert_generation();

    let unavailable = WorkerExecutorError::oplog_read_unavailable("primary read timed out");
    let waiting = tokio::spawn(worker.clone().await_initialization());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    worker.events().publish(Event::InvocationCompleted {
        agent_id: worker.agent_id(),
        idempotency_key: key,
        result: Box::new(Err(unavailable.clone())),
    });
    assert_eq!(
        within_five_seconds(
            "a wait whose notification is a read error must end with it",
            waiting
        )
        .await
        .unwrap(),
        Err(unavailable)
    );

    worker.hydrated_invocation_results.write().await.insert(
        key,
        InvocationResult::Cached {
            result: Err(FailedInvocationResult::Trap {
                trap_type: TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::now_utc())),
                stderr: String::new(),
            }),
        },
        generation,
        tail,
    );
    let waiting = tokio::spawn(worker.clone().await_initialization());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        waiting.is_finished(),
        "an interrupted wait must end at once, not wait again"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail);
    // A success that the agent records later does not replace the interruption of the wait.
    resident
        .oplog
        .add(OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
            result: OplogPayload::SerializedInline {
                bytes: golem_common::serialization::serialize(
                    &AgentInvocationResult::AgentInitialization,
                )
                .unwrap(),
                cached: None,
            },
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision::INITIAL,
            awaits_settlement: false,
        })
        .await
        .unwrap();
    worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await
        .unwrap();
    let interrupted = within_five_seconds("the wait must end", waiting)
        .await
        .unwrap();
    assert!(
        matches!(
            interrupted,
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Interrupt(_)
            })
        ),
        "the wait ends as interrupted: {interrupted:?}"
    );
    resident.stop().await;
}

/// A waiter whose notification is a read error that failed for a time answers with a result that
/// settled while it waited, and with the read error when no result settled.
#[test]
#[test_r::timeout("60s")]
async fn a_classified_notification_gives_way_to_a_result_that_settled_during_the_wait() {
    use crate::model::LookupResult;
    use crate::services::events::Event;

    let resident = ResidentWorker::start().await;
    let worker = resident.worker.clone();
    let tail = resident.oplog.current_oplog_index().await;
    let generation = worker
        .try_get_repaired_status()
        .await
        .unwrap()
        .invocation_results
        .revert_generation();
    let unavailable = WorkerExecutorError::oplog_read_unavailable("primary read timed out");
    let outcomes = futures::stream::iter([true, false])
        .then(|settled| {
            let worker = worker.clone();
            let unavailable = unavailable.clone();
            async move {
                let key = InvocationId(uuid::Uuid::new_v4());
                assert_eq!(
                    worker.try_lookup_invocation_result(&key).await.unwrap(),
                    LookupResult::New
                );
                let waiting = tokio::spawn({
                    let worker = worker.clone();
                    let subscription = worker.events().subscribe();
                    async move { worker.wait_for_invocation_result(&key, subscription).await }
                });
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if settled {
                    worker.hydrated_invocation_results.write().await.insert(
                        key,
                        InvocationResult::Cached {
                            result: Ok(cached_output(7)),
                        },
                        generation,
                        tail,
                    );
                }
                worker.events().publish(Event::InvocationCompleted {
                    agent_id: worker.agent_id(),
                    idempotency_key: key,
                    result: Box::new(Err(unavailable)),
                });
                (
                    settled,
                    within_five_seconds("the waiter must answer the notification", waiting)
                        .await
                        .unwrap(),
                )
            }
        })
        .collect::<Vec<_>>()
        .await;
    outcomes.into_iter().for_each(|(settled, outcome)| {
        if settled {
            assert_eq!(outcome, Ok(LookupResult::Complete(Ok(cached_output(7)))));
        } else {
            assert_eq!(outcome, Err(unavailable.clone()));
        }
    });
    assert_eq!(resident.oplog.current_oplog_index().await, tail);
    resident.stop().await;
}

/// A wait for an initialization whose result the agent recorded ends at once with the success,
/// before it starts the agent or waits for a notification. A failure notification that arrives
/// later does not replace the recorded success.
#[test]
#[test_r::timeout("60s")]
async fn an_initialization_wait_with_a_recorded_result_ends_at_once() {
    use crate::services::events::Event;

    let resident = ResidentWorker::start().await;
    let worker = resident.worker.clone();
    let _commands = resident.install_running().await;
    let key = InvocationId::constructor(worker.initial_worker_metadata.fingerprint);
    resident
        .oplog
        .add(OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
            result: OplogPayload::SerializedInline {
                bytes: golem_common::serialization::serialize(
                    &AgentInvocationResult::AgentInitialization,
                )
                .unwrap(),
                cached: None,
            },
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision::INITIAL,
            awaits_settlement: false,
        })
        .await
        .unwrap();
    worker
        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
        .await
        .unwrap();
    assert_eq!(
        worker.recorded_initialization_result(&key).await,
        Ok(true),
        "the recorded initialization success is the result of the initialization"
    );
    let waiting = tokio::spawn(worker.clone().await_initialization());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    worker.events().publish(Event::InvocationCompleted {
        agent_id: worker.agent_id(),
        idempotency_key: key,
        result: Box::new(Err(WorkerExecutorError::runtime("constructor failed"))),
    });
    assert_eq!(
        within_five_seconds("a recorded initialization must end the wait", waiting)
            .await
            .unwrap(),
        Ok(())
    );
    resident.stop().await;
}

/// A queued method that starts a durable stream session, sent again after the agent retained its
/// attachment, reuses that attachment when it repeats the attempt and is an attempt conflict when
/// it is another attempt. Neither appends an invocation or an attachment again.
#[test]
#[test_r::timeout("60s")]
async fn a_repeated_queued_stream_method_reuses_its_attachment_and_refuses_another_attempt() {
    use golem_common::model::durable_stream::AttemptId;

    let resident = ResidentWorker::start().await;
    let worker = resident.worker.clone();
    let _commands = resident.install_running().await;
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key: InvocationId(uuid::Uuid::from_u128(0x1628_0242)),
        method_name: "count".into(),
        allow_streams: true,
        input: golem_common::schema::SchemaValue::Record { fields: Vec::new() }
            .try_into()
            .unwrap(),
        invocation_context: golem_common::model::invocation_context::InvocationContextStack::fresh(
        ),
        caller: golem_common::model::caller::Caller::anonymous(),
        scope_card: None,
    };
    let acceptance = crate::grpc::capture_session_acceptance(
        &golem_api_grpc::proto::golem::worker::InvocationStart::default(),
        8,
    )
    .unwrap();
    let attempt = AttemptId(uuid::Uuid::new_v4());
    let accept = |attempt| {
        let worker = worker.clone();
        let invocation = invocation.clone();
        let acceptance = acceptance.clone();
        async move {
            let (committed, notified) = tokio::sync::oneshot::channel();
            let accepted = worker
                .accept_queued_method(invocation, acceptance, attempt, committed)
                .await;
            (accepted, notified.await.is_ok())
        }
    };

    let (first, first_committed) = accept(attempt).await;
    let (attached, replayed) = first.unwrap();
    assert!(!replayed && first_committed);
    let tail = resident.oplog.current_oplog_index().await;

    let (again, again_committed) = accept(attempt).await;
    assert_eq!(
        again.unwrap(),
        (attached, true),
        "the same attempt reuses the retained attachment"
    );
    assert!(again_committed);

    let (other, other_committed) = accept(AttemptId(uuid::Uuid::new_v4())).await;
    assert!(
        matches!(&other, Err(WorkerExecutorError::InvalidRequest { details })
            if details.starts_with("AttemptConflict:")),
        "another attempt conflicts with the retained attachment: {other:?}"
    );
    assert!(!other_committed);
    assert_eq!(resident.oplog.current_oplog_index().await, tail);
    resident.stop().await;
}

/// A durable session start sent again after the agent prepared its session reuses the prepared
/// attempt when it repeats that attempt, and is an attempt conflict when it is another attempt.
/// A stored execution configuration that does not decode is a read error, not a conflict. None of
/// them prepares the session or appends an invocation again.
#[test]
#[test_r::timeout("60s")]
async fn a_repeated_durable_session_start_reuses_its_prepared_attempt_and_refuses_another() {
    use golem_api_grpc::proto::golem::worker::InvocationStart;
    use golem_common::model::IdempotencyKey;
    use golem_common::model::agent::{AgentTypeName, Snapshotting};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::schema::SchemaValue;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
        OutputSchema,
    };

    let resident = ResidentWorker::start().await;
    let worker = resident.worker.clone();
    let _commands = resident.install_running().await;
    let agent_type = AgentTypeName("Counter".into());
    let metadata = ComponentMetadata::from_parts(
        Default::default(),
        Vec::new(),
        vec![AgentTypeSchema {
            type_name: agent_type.clone(),
            kind: AgentTypeKind::Regular,
            description: String::new(),
            source_language: String::new(),
            schema: golem_common::schema::SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: vec![AgentMethodSchema {
                name: "count".into(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
                output_schema: OutputSchema::Unit,
                http_endpoint: Vec::new(),
                read_only: None,
            }],
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
            config: Vec::new(),
        }],
        Default::default(),
    );
    let key = InvocationId(uuid::Uuid::from_u128(0x1628_0249));
    let callee = worker.owned_agent_id.agent_id;
    let fingerprint = worker.initial_worker_metadata.fingerprint;
    let start = |key: InvocationId, attempt: uuid::Uuid, stored_config: Option<Vec<u8>>| {
        let worker = worker.clone();
        let metadata = metadata.clone();
        let agent_type = agent_type.clone();
        async move {
            let request = InvocationStart {
                method_name: Some("count".into()),
                idempotency_key: Some(IdempotencyKey::from_uuid(key.0).into()),
                environment_id: Some(worker.owned_agent_id.environment_id.into()),
                attempt_id: Some(attempt.into()),
                expected_callee_fingerprint: Some(fingerprint.0.into()),
                input: Some(
                    golem_common::schema::streaming::encode_recursive_stream_value(
                        &SchemaValue::Record { fields: Vec::new() },
                        |_, _| unreachable!("the input has no stream"),
                    )
                    .unwrap(),
                ),
                ..Default::default()
            };
            let invocation = AgentInvocation::AgentMethod {
                idempotency_key: key,
                method_name: "count".into(),
                allow_streams: true,
                input: crate::grpc::capture_method_input(&request).unwrap(),
                invocation_context:
                    golem_common::model::invocation_context::InvocationContextStack::fresh(),
                caller: golem_common::model::caller::Caller::anonymous(),
                scope_card: None,
            };
            let (committed, notified) = tokio::sync::oneshot::channel();
            let mut durable = crate::grpc::build_durable_streaming_request(
                &request,
                callee,
                Some(&agent_type),
                &metadata,
                ComponentRevision::INITIAL,
                fingerprint,
                invocation,
                0,
                committed,
                8,
            )
            .unwrap();
            if let Some(config) = stored_config {
                durable.attempt.invocation.execution_config = config;
            }
            let accepted = worker
                .accept_durable_streaming_invocation(durable)
                .await
                .map(|acceptance| (acceptance.replayed, acceptance.prepared.attempt.attempt_id));
            (accepted, notified.await.is_ok())
        }
    };
    let attempt = uuid::Uuid::new_v4();

    let (first, first_committed) = start(key, attempt, None).await;
    assert_eq!(
        first.unwrap(),
        (
            false,
            golem_common::model::durable_stream::AttemptId(attempt)
        )
    );
    assert!(first_committed);
    let tail = resident.oplog.current_oplog_index().await;

    let (again, _) = start(key, attempt, None).await;
    assert_eq!(
        again.unwrap(),
        (
            true,
            golem_common::model::durable_stream::AttemptId(attempt)
        ),
        "the same attempt reuses the prepared session"
    );
    assert_eq!(resident.oplog.current_oplog_index().await, tail);

    let (other, other_committed) = start(key, uuid::Uuid::new_v4(), None).await;
    let error = other.err();
    assert!(
        matches!(&error, Some(WorkerExecutorError::InvalidRequest { details })
            if details.starts_with("AttemptConflict:")),
        "another attempt conflicts with the prepared session: {error:?}"
    );
    assert!(!other_committed);
    assert_eq!(resident.oplog.current_oplog_index().await, tail);

    // A retry of a session whose stored execution configuration does not decode differs from it
    // only in that configuration. The stored configuration is a read error of its entry, not a
    // conflict with the request.
    let unreadable = InvocationId(uuid::Uuid::from_u128(0x1628_0249_0002));
    let attempt = uuid::Uuid::new_v4();
    let (stored, stored_committed) = start(unreadable, attempt, Some(vec![99])).await;
    assert_eq!(stored.map(|(replayed, _)| replayed).ok(), Some(false));
    assert!(stored_committed);
    let tail = resident.oplog.current_oplog_index().await;
    let (read, read_committed) = start(unreadable, attempt, None).await;
    assert!(
        matches!(&read, Err(WorkerExecutorError::OplogFailed { agent_id, details, .. })
            if *agent_id == worker.owned_agent_id.agent_id
                && details.contains("stream execution config")),
        "an undecodable stored configuration is a read error, not a conflict: {read:?}"
    );
    assert!(!read_committed);
    assert_eq!(resident.oplog.current_oplog_index().await, tail);
    resident.stop().await;
}
