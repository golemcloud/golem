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
use crate::sharding::model::{ShardEpoch, incarnation_id};
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::model::sharding::{
    ShardAssignmentEvent, ShardAssignmentState, transition_shard_assignment,
};
use golem_common::model::{RetryConfig, ShardAssignment};
use golem_common::retriable_error::IsRetriableError;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::clients::shard_manager::{ExecutorEndpoint, ShardManager};
use golem_service_base::config::BlobStorageConfig;
use golem_worker_executor::services::NoAdditionalDeps;
use golem_worker_executor::services::golem_config::*;
use golem_worker_executor::services::quota::{QuotaService, UnlimitedQuotaService};
use golem_worker_executor::services::shard::{ShardService, ShardServiceDefault};
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::shutdown::{SHUTDOWN_GRACE, Shutdown};
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::storage::keyvalue::{KeyValueStorage, KeyValueStorageError};
use golem_worker_executor::workerctx::default::Context;
use golem_worker_executor::{Bootstrap, RunDetails, bootstrap_and_run_worker_executor};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::server::NamedService;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Adapter {
    Memory,
    Grpc,
}

struct Fixture {
    service: Arc<dyn WorkerExecutorService>,
    pod: Pod,
    snapshot: Box<dyn Fn() -> Option<ShardAssignment> + Send + Sync>,
    set_index_outage: Box<dyn Fn(bool) + Send + Sync>,
    runtime: ExecutorRuntime,
}

const INDEX_OUTAGE: &str = "injected recovery index outage";

impl Fixture {
    async fn new(adapter: Adapter, state: ShardAssignmentState) -> Self {
        match adapter {
            Adapter::Memory => {
                let pod = Pod {
                    ip: "127.0.0.1".parse().unwrap(),
                    port: 1,
                };
                let service = Arc::new(InMemoryWorkerExecutorService::new([(pod, state)]));
                let observed = service.clone();
                let controlled = service.clone();
                Self {
                    service,
                    pod,
                    snapshot: Box::new(move || observed.current_assignment(&pod)),
                    set_index_outage: Box::new(move |outage| {
                        controlled
                            .set_conditions(
                                &pod,
                                InMemoryWorkerExecutorConditions {
                                    recovery_index_error: outage.then(|| INDEX_OUTAGE.into()),
                                    ..Default::default()
                                },
                            )
                            .unwrap();
                    }),
                    runtime: ExecutorRuntime::default(),
                }
            }
            Adapter::Grpc => {
                let _ = rustls::crypto::ring::default_provider().install_default();
                let shard_service = Arc::new(ShardServiceDefault::new());
                let bootstrap = ContractBootstrap {
                    shard_service: shard_service.clone(),
                    registration: Arc::new(SeedRegistration(state.as_ref().unwrap().clone())),
                    faults: KeyValueStorageFaults::default(),
                };
                let mut runtime = ExecutorRuntime::default();
                let pod = runtime.start_executor(&bootstrap).await;
                let faults = bootstrap.faults;
                Self {
                    service: Arc::new(WorkerExecutorServiceDefault::new(client_config())),
                    pod,
                    snapshot: Box::new(move || shard_service.try_get_current_assignment()),
                    set_index_outage: Box::new(move |outage| {
                        if outage {
                            faults.fail(
                                "enum",
                                usize::MAX,
                                KeyValueStorageError::Other(INDEX_OUTAGE.into()),
                            );
                        } else {
                            faults.clear();
                        }
                    }),
                    runtime,
                }
            }
        }
    }

    async fn finish(mut self) {
        drop(self.service);
        self.runtime.cleanup().unwrap().await.unwrap();
    }
}

fn client_config() -> WorkerExecutorServiceConfig {
    let mut config = WorkerExecutorServiceConfig {
        retries: RetryConfig::no_retries(),
        ..Default::default()
    };
    config.client_config.retries_on_unavailable = RetryConfig::no_retries();
    config
}

fn local_executor_config() -> GolemConfig {
    let mut config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        scheduler_storage: SchedulerStorageConfig::InMemory(SchedulerStorageInMemoryConfig {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        resource_limits: ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
        filesystem_snapshots: FilesystemSnapshotsConfig::Disabled(
            FilesystemSnapshotsDisabledConfig {},
        ),
        bind_address: "127.0.0.1".parse().unwrap(),
        http_port: 0,
        ..GolemConfig::default()
    };
    config.grpc.port = 0;
    config.filesystem_storage.mode = FilesystemStorageMode::Temporary;
    config.engine.enable_fs_cache = false;
    config.set_golem_issuer(GolemIssuer::generate());
    config
}

struct SeedRegistration(ShardAssignment);

#[async_trait]
impl ShardManagerService for SeedRegistration {
    async fn register(
        &self,
        _endpoint: ExecutorEndpoint,
        _pod_name: Option<String>,
    ) -> Result<ShardAssignment, golem_service_base::clients::shard_manager::ShardManagerError>
    {
        Ok(self.0.clone())
    }

    async fn deregister(&self) {}
}

struct ContractBootstrap {
    shard_service: Arc<ShardServiceDefault>,
    registration: Arc<SeedRegistration>,
    faults: KeyValueStorageFaults,
}

#[async_trait]
impl Bootstrap<Context> for ContractBootstrap {
    fn create_additional_deps(&self, _registry: Arc<dyn RegistryService>) -> NoAdditionalDeps {
        NoAdditionalDeps::new()
    }

    fn create_shard_service(&self) -> Arc<dyn ShardService> {
        self.shard_service.clone()
    }

    fn create_shard_manager_service(
        &self,
        _client: Arc<dyn ShardManager>,
        _shards: Arc<dyn ShardService>,
        _shutdown: Shutdown,
    ) -> Arc<dyn ShardManagerService> {
        self.registration.clone()
    }

    fn create_quota_service(
        &self,
        _client: Arc<dyn ShardManager>,
        _config: &QuotaServiceConfig,
        _shutdown: CancellationToken,
    ) -> Arc<dyn QuotaService> {
        Arc::new(UnlimitedQuotaService)
    }

    fn wrap_key_value_storage(
        &self,
        storage: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Arc<dyn KeyValueStorage + Send + Sync> {
        Arc::new(FaultInjectingKeyValueStorage::new(
            storage,
            self.faults.clone(),
        ))
    }
}

enum ExecutorRuntimePhase {
    Preparing(JoinSet<anyhow::Result<()>>),
    Running {
        run: Box<RunDetails>,
        tasks: JoinSet<anyhow::Result<()>>,
    },
    Released,
}

enum ExecutorRuntimeEvent {
    Started(Box<RunDetails>),
    Release,
}

struct ExecutorCleanup {
    run: Option<Box<RunDetails>>,
    tasks: JoinSet<anyhow::Result<()>>,
}

fn transition_executor_runtime(
    phase: ExecutorRuntimePhase,
    event: ExecutorRuntimeEvent,
) -> (ExecutorRuntimePhase, Option<ExecutorCleanup>) {
    match (phase, event) {
        (ExecutorRuntimePhase::Preparing(tasks), ExecutorRuntimeEvent::Started(run)) => {
            (ExecutorRuntimePhase::Running { run, tasks }, None)
        }
        (ExecutorRuntimePhase::Preparing(tasks), ExecutorRuntimeEvent::Release) => (
            ExecutorRuntimePhase::Released,
            Some(ExecutorCleanup { run: None, tasks }),
        ),
        (ExecutorRuntimePhase::Running { run, tasks }, ExecutorRuntimeEvent::Release) => (
            ExecutorRuntimePhase::Released,
            Some(ExecutorCleanup {
                run: Some(run),
                tasks,
            }),
        ),
        (ExecutorRuntimePhase::Released, ExecutorRuntimeEvent::Release) => {
            (ExecutorRuntimePhase::Released, None)
        }
        (_, ExecutorRuntimeEvent::Started(_)) => {
            unreachable!("executor already started or released")
        }
    }
}

struct ExecutorRuntime {
    phase: ExecutorRuntimePhase,
}

impl Default for ExecutorRuntime {
    fn default() -> Self {
        Self {
            phase: ExecutorRuntimePhase::Preparing(JoinSet::new()),
        }
    }
}

impl ExecutorRuntime {
    fn apply(&mut self, event: ExecutorRuntimeEvent) -> Option<ExecutorCleanup> {
        let phase = std::mem::replace(&mut self.phase, ExecutorRuntimePhase::Released);
        let (next, cleanup) = transition_executor_runtime(phase, event);
        self.phase = next;
        cleanup
    }

    async fn start_executor(&mut self, bootstrap: &ContractBootstrap) -> Pod {
        let ExecutorRuntimePhase::Preparing(tasks) = &mut self.phase else {
            unreachable!("executor already started or released")
        };
        let run = bootstrap_and_run_worker_executor(
            bootstrap,
            local_executor_config(),
            prometheus::Registry::new(),
            tokio::runtime::Handle::current(),
            tasks,
            false,
        )
        .await
        .unwrap();
        let pod = Pod {
            ip: "127.0.0.1".parse().unwrap(),
            port: run.grpc_port,
        };
        drop(self.apply(ExecutorRuntimeEvent::Started(Box::new(run))));
        pod
    }

    async fn start_health(
        &mut self,
        health: InMemoryWorkerExecutorHealth,
        entered: Arc<Notify>,
        release: CancellationToken,
    ) -> Pod {
        let ExecutorRuntimePhase::Preparing(tasks) = &mut self.phase else {
            unreachable!("executor already started or released")
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (reporter, service) = tonic_health::server::health_reporter();
        reporter
            .set_service_status(
                "",
                match health {
                    InMemoryWorkerExecutorHealth::NotServing => {
                        tonic_health::ServingStatus::NotServing
                    }
                    _ => tonic_health::ServingStatus::Serving,
                },
            )
            .await;
        let service = HealthResponseGate {
            inner: service,
            entered,
            release,
        };
        tasks.spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .map_err(anyhow::Error::from)
        });
        Pod {
            ip: address.ip(),
            port: address.port(),
        }
    }

    fn observe_shutdown(&mut self) -> impl Future<Output = ()> + use<> {
        let ExecutorRuntimePhase::Running { run, tasks } = &mut self.phase else {
            unreachable!("executor is not running")
        };
        let stopped = CancellationToken::new();
        let signal = stopped.clone().drop_guard();
        tasks.spawn(async move {
            let _signal = signal;
            std::future::pending::<anyhow::Result<()>>().await
        });
        let shutdown = run.shutdown.clone();
        async move {
            assert!(shutdown.token().is_cancelled());
            stopped.cancelled().await;
            shutdown.join_tracked().await;
        }
    }

    fn cleanup(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.apply(ExecutorRuntimeEvent::Release)
            .map(ExecutorCleanup::start)
    }
}

impl ExecutorCleanup {
    fn start(self) -> tokio::task::JoinHandle<()> {
        let Self { run, mut tasks } = self;
        if let Some(run) = run.as_ref() {
            run.shutdown.cancel();
        }
        tokio::spawn(async move {
            let drained = match run.as_ref() {
                Some(run) => run.shutdown.wait_for_tracked(SHUTDOWN_GRACE).await,
                None => true,
            };
            tasks.shutdown().await;
            drop(run);
            assert!(drained, "executor tracked shutdown did not finish");
        })
    }
}

impl Drop for ExecutorRuntime {
    fn drop(&mut self) {
        drop(self.cleanup());
    }
}

fn registered(incarnation: Uuid) -> ShardAssignmentState {
    registered_until(incarnation, Instant::now() + Duration::from_secs(60))
}

fn registered_until(incarnation: Uuid, expires_at: Instant) -> ShardAssignmentState {
    let shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(7)),
        (ShardId::new(1), golem_common::model::ShardEpoch(8)),
    ]);
    transition_shard_assignment(
        ShardAssignmentState::default(),
        ShardAssignmentEvent::Register {
            number_of_shards: 8,
            shard_epochs: &shard_epochs,
            expires_at: Some(expires_at),
            revision: golem_common::model::ShardLeaseRevision {
                incarnation,
                number: 1,
            },
            executor_count: NonZeroUsize::new(2).unwrap(),
        },
    )
    .0
}

fn assignment(revision: u64, executor_count: usize) -> ShardAssignmentPush {
    ShardAssignmentPush {
        number_of_shards: 16,
        shard_epochs: BTreeMap::from([
            (ShardId::new(0), ShardEpoch(9)),
            (ShardId::new(2), ShardEpoch(4)),
        ]),
        executor_count,
        revision: ShardLeaseRevision(revision),
    }
}

fn assert_snapshot(fixture: &Fixture, expected: &ShardAssignment) {
    let actual = (fixture.snapshot)().expect("registered assignment");
    assert_assignment(&actual, expected);
}

fn assert_assignment(actual: &ShardAssignment, expected: &ShardAssignment) {
    assert_eq!(actual.number_of_shards, expected.number_of_shards);
    assert_eq!(actual.executor_count, expected.executor_count);
    assert_eq!(actual.shard_epochs, expected.shard_epochs);
    assert_eq!(actual.revision, expected.revision);
    assert_eq!(actual.expires_at, expected.expires_at);
}

fn assert_worker_error(result: Result<(), ShardManagerError>, expected: WorkerExecutorError) {
    match result {
        Err(ShardManagerError::WorkerExecutionError(actual)) => assert_eq!(actual, expected),
        other => panic!("expected {expected:?}, got {other:?}"),
    }
}

async fn delivery_contract(adapter: Adapter) {
    let incarnation = Uuid::parse_str(&incarnation_id()).unwrap();
    let fixture = Fixture::new(adapter, registered(incarnation)).await;
    let expiry = (fixture.snapshot)().unwrap().expires_at;
    fixture.service.health_check(&fixture.pod).await.unwrap();

    fixture
        .service
        .assign_shards(&fixture.pod, &assignment(10, 5))
        .await
        .unwrap();
    let mut expected = (fixture.snapshot)().unwrap();
    assert_eq!(expected.number_of_shards, 16);
    assert_eq!(expected.executor_count, 5);
    assert_eq!(expected.expires_at, expiry);
    assert_eq!(expected.revision.unwrap().number, 10);
    assert_eq!(expected.revision.unwrap().incarnation, incarnation);
    assert_eq!(
        expected.held_epochs(),
        BTreeMap::from([
            (ShardId::new(0), golem_common::model::ShardEpoch(9)),
            (ShardId::new(2), golem_common::model::ShardEpoch(4)),
        ])
    );

    (fixture.set_index_outage)(true);
    let mut stale = assignment(9, 8);
    stale.shard_epochs.clear();
    fixture
        .service
        .assign_shards(&fixture.pod, &stale)
        .await
        .unwrap();
    assert_snapshot(&fixture, &expected);
    let removed = BTreeSet::from([ShardId::new(0)]);
    fixture
        .service
        .revoke_shards(&fixture.pod, &removed, ShardLeaseRevision(9), 8)
        .await
        .unwrap();
    assert_snapshot(&fixture, &expected);

    (fixture.set_index_outage)(false);
    fixture
        .service
        .assign_shards(&fixture.pod, &assignment(10, 4))
        .await
        .unwrap();
    expected.executor_count = 4;
    assert_snapshot(&fixture, &expected);

    fixture
        .service
        .revoke_shards(&fixture.pod, &removed, ShardLeaseRevision(11), 3)
        .await
        .unwrap();
    expected.shard_epochs.remove(&ShardId::new(0));
    expected.executor_count = 3;
    expected.revision.as_mut().unwrap().number = 11;
    assert_snapshot(&fixture, &expected);

    fixture
        .service
        .revoke_shards(&fixture.pod, &BTreeSet::new(), ShardLeaseRevision(12), 2)
        .await
        .unwrap();
    expected.executor_count = 2;
    expected.revision.as_mut().unwrap().number = 12;
    assert_snapshot(&fixture, &expected);

    assert_worker_error(
        fixture
            .service
            .assign_shards(&fixture.pod, &assignment(13, 0))
            .await,
        WorkerExecutorError::invalid_request("AssignShardsRequest.executor_count must not be 0"),
    );
    assert_snapshot(&fixture, &expected);
    assert_worker_error(
        fixture
            .service
            .revoke_shards(
                &fixture.pod,
                &BTreeSet::from([ShardId::new(2)]),
                ShardLeaseRevision(13),
                0,
            )
            .await,
        WorkerExecutorError::invalid_request("RevokeShardsRequest.executor_count must not be 0"),
    );
    assert_snapshot(&fixture, &expected);
    let mut invalid = assignment(13, 3);
    invalid.number_of_shards = 0;
    assert_worker_error(
        fixture.service.assign_shards(&fixture.pod, &invalid).await,
        WorkerExecutorError::invalid_request("AssignShardsRequest.number_of_shards must not be 0"),
    );
    assert_snapshot(&fixture, &expected);

    let mut empty = assignment(13, 3);
    empty.shard_epochs.clear();
    fixture
        .service
        .assign_shards(&fixture.pod, &empty)
        .await
        .unwrap();
    expected.shard_epochs.clear();
    expected.executor_count = 3;
    expected.revision.as_mut().unwrap().number = 13;
    assert_snapshot(&fixture, &expected);
    assert_invalid_deliveries(&fixture, &fixture.pod).await;

    let maximum = usize::try_from(u32::MAX).unwrap();
    fixture
        .service
        .assign_shards(&fixture.pod, &assignment(14, maximum))
        .await
        .unwrap();
    expected.shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(9)),
        (ShardId::new(2), golem_common::model::ShardEpoch(4)),
    ]);
    expected.executor_count = maximum;
    expected.revision.as_mut().unwrap().number = 14;
    assert_snapshot(&fixture, &expected);
    fixture
        .service
        .revoke_shards(&fixture.pod, &removed, ShardLeaseRevision(15), maximum)
        .await
        .unwrap();
    expected.shard_epochs.remove(&ShardId::new(0));
    expected.revision.as_mut().unwrap().number = 15;
    assert_snapshot(&fixture, &expected);
    fixture.finish().await;

    let other_manager = Uuid::from_u128(0xf00);
    assert_ne!(other_manager, incarnation);
    let fixture = Fixture::new(adapter, registered(other_manager)).await;
    let expected = (fixture.snapshot)().unwrap();
    (fixture.set_index_outage)(true);
    fixture
        .service
        .assign_shards(&fixture.pod, &assignment(100, 9))
        .await
        .unwrap();
    assert_snapshot(&fixture, &expected);
    fixture
        .service
        .revoke_shards(&fixture.pod, &removed, ShardLeaseRevision(100), 9)
        .await
        .unwrap();
    assert_snapshot(&fixture, &expected);
    fixture.finish().await;

    let fixture = Fixture::new(adapter, registered(incarnation)).await;
    let reserved = tokio::net::TcpSocket::new_v4().unwrap();
    reserved.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = reserved.local_addr().unwrap();
    let unreachable = Pod {
        ip: address.ip(),
        port: address.port(),
    };
    assert_invalid_deliveries(&fixture, &unreachable).await;
    assert_unavailable(
        fixture
            .service
            .assign_shards(&unreachable, &assignment(20, 3))
            .await,
    );
    assert_unavailable(
        fixture
            .service
            .revoke_shards(&unreachable, &removed, ShardLeaseRevision(20), 3)
            .await,
    );
    let health = fixture
        .service
        .health_check(&unreachable)
        .await
        .unwrap_err();
    assert!(health.is_retriable());
    match health {
        HealthCheckError::GrpcError(status) => assert_eq!(status.code(), tonic::Code::Unavailable),
        other => panic!("expected health Unavailable, got {other:?}"),
    }
    fixture.finish().await;
}

fn assert_unchanged(fixture: &Fixture, expected: Option<&ShardAssignment>) {
    match expected {
        Some(expected) => assert_snapshot(fixture, expected),
        None => assert!((fixture.snapshot)().is_none()),
    }
}

fn assert_unavailable(result: Result<(), ShardManagerError>) {
    let error = result.unwrap_err();
    assert!(error.is_retriable());
    match error {
        ShardManagerError::GrpcError(status) => assert_eq!(status.code(), tonic::Code::Unavailable),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

async fn assert_invalid_deliveries(fixture: &Fixture, target: &Pod) {
    use futures::StreamExt;

    let expected = (fixture.snapshot)();
    #[cfg(target_pointer_width = "64")]
    let counts = [0, 4_294_967_296usize, 4_294_967_299];
    #[cfg(not(target_pointer_width = "64"))]
    let counts = [0];
    futures::stream::iter(counts)
        .then(|count| {
            let expected = expected.as_ref();
            async move {
                let detail = |field| {
                    if count == 0 {
                        format!("{field}.executor_count must not be 0")
                    } else {
                        format!("{field}.executor_count {count} does not fit a u32")
                    }
                };
                assert_worker_error(
                    fixture
                        .service
                        .assign_shards(target, &assignment(30, count))
                        .await,
                    WorkerExecutorError::invalid_request(detail("AssignShardsRequest")),
                );
                assert_unchanged(fixture, expected);
                assert_worker_error(
                    fixture
                        .service
                        .revoke_shards(
                            target,
                            &BTreeSet::from([ShardId::new(0)]),
                            ShardLeaseRevision(30),
                            count,
                        )
                        .await,
                    WorkerExecutorError::invalid_request(detail("RevokeShardsRequest")),
                );
                assert_unchanged(fixture, expected);
            }
        })
        .collect::<()>()
        .await;
    let mut invalid = assignment(30, 3);
    invalid.number_of_shards = 0;
    assert_worker_error(
        fixture.service.assign_shards(target, &invalid).await,
        WorkerExecutorError::invalid_request("AssignShardsRequest.number_of_shards must not be 0"),
    );
    assert_unchanged(fixture, expected.as_ref());
}

#[test]
#[timeout("30s")]
async fn in_memory_worker_executor_obeys_delivery_contract() {
    delivery_contract(Adapter::Memory).await;
}

#[test]
#[timeout("30s")]
async fn grpc_worker_executor_obeys_delivery_contract() {
    delivery_contract(Adapter::Grpc).await;
}

#[test]
#[timeout("30s")]
async fn cancelling_the_grpc_fixture_owner_stops_its_executor_and_servers() {
    let mut fixture = Fixture::new(
        Adapter::Grpc,
        registered(Uuid::parse_str(&incarnation_id()).unwrap()),
    )
    .await;
    let shutdown = fixture.runtime.observe_shutdown();
    let owner = tokio::spawn(async move {
        let _fixture = fixture;
        std::future::pending::<()>().await;
    });
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    shutdown.await;
}

async fn recovery_index_outage_contract(adapter: Adapter) {
    let incarnation = Uuid::parse_str(&incarnation_id()).unwrap();
    let initial = registered(incarnation);
    let initial_assignment = initial.as_ref().unwrap().clone();
    let fixture = Fixture::new(adapter, initial).await;
    assert_snapshot(&fixture, &initial_assignment);
    (fixture.set_index_outage)(true);
    assert_snapshot(&fixture, &initial_assignment);
    let expected = ShardAssignment {
        number_of_shards: 16,
        executor_count: 5,
        shard_epochs: HashMap::from([
            (ShardId::new(0), golem_common::model::ShardEpoch(9)),
            (ShardId::new(2), golem_common::model::ShardEpoch(4)),
        ]),
        revision: Some(golem_common::model::ShardLeaseRevision {
            incarnation,
            number: 10,
        }),
        expires_at: initial_assignment.expires_at,
    };
    let push = assignment(10, 5);
    assert_index_outage(fixture.service.assign_shards(&fixture.pod, &push).await);
    assert_snapshot(&fixture, &expected);
    assert_index_outage(fixture.service.assign_shards(&fixture.pod, &push).await);
    assert_snapshot(&fixture, &expected);
    (fixture.set_index_outage)(false);
    assert_snapshot(&fixture, &expected);
    fixture
        .service
        .assign_shards(&fixture.pod, &push)
        .await
        .unwrap();
    assert_snapshot(&fixture, &expected);
    fixture.finish().await;
}

fn assert_index_outage(result: Result<(), ShardManagerError>) {
    assert!(result.as_ref().unwrap_err().is_retriable());
    assert_worker_error(
        result,
        WorkerExecutorError::Runtime {
            details: "failed to get worker ids from KV storage: injected recovery index outage"
                .into(),
        },
    );
}

#[test]
#[timeout("30s")]
async fn grpc_assignment_installs_before_a_persistent_recovery_index_failure() {
    recovery_index_outage_contract(Adapter::Grpc).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_assignment_installs_before_a_persistent_recovery_index_failure() {
    recovery_index_outage_contract(Adapter::Memory).await;
}

async fn recovery_index_controls_contract(adapter: Adapter) {
    let incarnation = Uuid::parse_str(&incarnation_id()).unwrap();
    let fixture = Fixture::new(adapter, registered(incarnation)).await;
    let mut expected = (fixture.snapshot)().unwrap();
    (fixture.set_index_outage)(true);
    assert_invalid_deliveries(&fixture, &fixture.pod).await;
    let omitted = Pod {
        port: 0,
        ..fixture.pod
    };
    assert_invalid_deliveries(&fixture, &omitted).await;
    fixture
        .service
        .revoke_shards(
            &fixture.pod,
            &BTreeSet::from([ShardId::new(0)]),
            ShardLeaseRevision(10),
            5,
        )
        .await
        .unwrap();
    expected.shard_epochs.remove(&ShardId::new(0));
    expected.executor_count = 5;
    expected.revision.as_mut().unwrap().number = 10;
    assert_snapshot(&fixture, &expected);
    let mut empty = assignment(11, 4);
    empty.shard_epochs.clear();
    fixture
        .service
        .assign_shards(&fixture.pod, &empty)
        .await
        .unwrap();
    expected.number_of_shards = 16;
    expected.shard_epochs.clear();
    expected.executor_count = 4;
    expected.revision.as_mut().unwrap().number = 11;
    assert_snapshot(&fixture, &expected);
    assert_index_outage(
        fixture
            .service
            .assign_shards(&fixture.pod, &assignment(12, 6))
            .await,
    );
    expected.shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(9)),
        (ShardId::new(2), golem_common::model::ShardEpoch(4)),
    ]);
    expected.executor_count = 6;
    expected.revision.as_mut().unwrap().number = 12;
    assert_snapshot(&fixture, &expected);
    fixture.finish().await;
}

#[test]
#[timeout("30s")]
async fn grpc_index_outage_does_not_fail_revoke_empty_or_invalid_deliveries() {
    recovery_index_controls_contract(Adapter::Grpc).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_index_outage_does_not_fail_revoke_empty_or_invalid_deliveries() {
    recovery_index_controls_contract(Adapter::Memory).await;
}

async fn lapsed_lease_contract(adapter: Adapter) {
    let incarnation = Uuid::parse_str(&incarnation_id()).unwrap();
    let expiry = Instant::now() - Duration::from_secs(1);
    let fixture = Fixture::new(adapter, registered_until(incarnation, expiry)).await;
    (fixture.set_index_outage)(true);
    fixture.service.health_check(&fixture.pod).await.unwrap();
    fixture
        .service
        .assign_shards(&fixture.pod, &assignment(10, 5))
        .await
        .unwrap();
    assert_snapshot(
        &fixture,
        &ShardAssignment {
            number_of_shards: 16,
            executor_count: 5,
            shard_epochs: HashMap::from([
                (ShardId::new(0), golem_common::model::ShardEpoch(9)),
                (ShardId::new(2), golem_common::model::ShardEpoch(4)),
            ]),
            revision: Some(golem_common::model::ShardLeaseRevision {
                incarnation,
                number: 10,
            }),
            expires_at: Some(expiry),
        },
    );
    fixture.service.health_check(&fixture.pod).await.unwrap();
    fixture.finish().await;
}

#[test]
#[timeout("30s")]
async fn grpc_lapsed_lease_defers_index_recovery_but_stays_healthy() {
    lapsed_lease_contract(Adapter::Grpc).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_lapsed_lease_defers_index_recovery_but_stays_healthy() {
    lapsed_lease_contract(Adapter::Memory).await;
}

async fn health_contract(adapter: Adapter, health: InMemoryWorkerExecutorHealth) {
    use futures::StreamExt;

    let mut runtime = ExecutorRuntime::default();
    let entered = Arc::new(Notify::new());
    let release = CancellationToken::new();
    let release_on_drop = release.clone().drop_guard();
    if health != InMemoryWorkerExecutorHealth::Timeout {
        release.cancel();
    }
    let (pod, service): (_, Arc<dyn WorkerExecutorService>) = match adapter {
        Adapter::Memory => {
            let pod = Pod {
                ip: "127.0.0.1".parse().unwrap(),
                port: 1,
            };
            let service =
                InMemoryWorkerExecutorService::new([(pod, ShardAssignmentState::default())]);
            service
                .set_conditions(
                    &pod,
                    InMemoryWorkerExecutorConditions {
                        health,
                        ..Default::default()
                    },
                )
                .unwrap();
            (pod, Arc::new(service))
        }
        Adapter::Grpc => {
            let pod = runtime.start_health(health, entered.clone(), release).await;
            let config = WorkerExecutorServiceConfig {
                health_check_timeout: Duration::from_secs(1),
                ..client_config()
            };
            (pod, Arc::new(WorkerExecutorServiceDefault::new(config)))
        }
    };
    futures::stream::iter([(), ()])
        .then(|()| async {
            let (result, ()) = tokio::join!(service.health_check(&pod), async {
                if matches!(adapter, Adapter::Grpc) {
                    entered.notified().await;
                }
            });
            assert_health(result, health);
        })
        .collect::<()>()
        .await;
    drop(service);
    drop(release_on_drop);
    runtime.cleanup().unwrap().await.unwrap();
}

#[derive(Clone)]
struct HealthResponseGate<S> {
    inner: S,
    entered: Arc<Notify>,
    release: CancellationToken,
}

impl<S: NamedService> NamedService for HealthResponseGate<S> {
    const NAME: &'static str = S::NAME;
}

impl<S> Service<http::Request<Body>> for HealthResponseGate<S>
where
    S: Service<http::Request<Body>>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let response = self.inner.call(request);
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            let response = response.await?;
            entered.notify_one();
            release.cancelled().await;
            Ok(response)
        })
    }
}

fn assert_health(result: Result<(), HealthCheckError>, health: InMemoryWorkerExecutorHealth) {
    match health {
        InMemoryWorkerExecutorHealth::Serving => result.unwrap(),
        InMemoryWorkerExecutorHealth::NotServing | InMemoryWorkerExecutorHealth::Timeout => {
            let expected = match health {
                InMemoryWorkerExecutorHealth::NotServing => "NOT_SERVING",
                _ => "health check timeout",
            };
            let error = result.unwrap_err();
            assert!(error.is_retriable());
            match error {
                HealthCheckError::GrpcOther(actual) => assert_eq!(actual, expected),
                other => panic!("expected {expected}, got {other:?}"),
            }
        }
    }
}

#[test]
#[timeout("30s")]
async fn grpc_health_reports_not_serving() {
    health_contract(Adapter::Grpc, InMemoryWorkerExecutorHealth::NotServing).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_health_reports_not_serving() {
    health_contract(Adapter::Memory, InMemoryWorkerExecutorHealth::NotServing).await;
}

#[test]
#[timeout("30s")]
async fn grpc_health_times_out_after_the_health_response_is_ready() {
    health_contract(Adapter::Grpc, InMemoryWorkerExecutorHealth::Timeout).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_health_times_out_after_the_health_response_is_ready() {
    health_contract(Adapter::Memory, InMemoryWorkerExecutorHealth::Timeout).await;
}

#[test]
async fn in_memory_delivery_before_registration_is_refused() {
    let fixture = Fixture::new(Adapter::Memory, ShardAssignmentState::default()).await;
    assert_invalid_deliveries(&fixture, &fixture.pod).await;
    assert_worker_error(
        fixture
            .service
            .assign_shards(&fixture.pod, &assignment(1, 3))
            .await,
        WorkerExecutorError::ShardingNotReady,
    );
    assert!((fixture.snapshot)().is_none());
    assert_worker_error(
        fixture
            .service
            .revoke_shards(
                &fixture.pod,
                &BTreeSet::from([ShardId::new(0)]),
                ShardLeaseRevision(1),
                3,
            )
            .await,
        WorkerExecutorError::ShardingNotReady,
    );
    assert!((fixture.snapshot)().is_none());
    fixture.finish().await;
}

fn registration() -> crate::sharding::model::RegisterAck {
    crate::sharding::model::RegisterAck {
        number_of_shards: 12,
        grant: crate::sharding::model::ShardLeaseGrant {
            shard_epochs: BTreeMap::from([(ShardId::new(3), ShardEpoch(19))]),
            expires_at: jiff::Timestamp::new(1_764_000_000, 0).unwrap(),
            executor_count: 4,
            revision: ShardLeaseRevision(7),
        },
    }
}

#[test]
#[timeout("30s")]
async fn in_memory_registration_installs_exact_fields_and_adopts_stale_expiry() {
    let pod = Pod {
        ip: "127.0.0.1".parse().unwrap(),
        port: 1,
    };
    let service = InMemoryWorkerExecutorService::new([(pod, ShardAssignmentState::default())]);
    let ack = registration();
    let now = Instant::now();
    let expiry = now - Duration::from_secs(1);
    service.install_registration(&pod, &ack, expiry).unwrap();
    let mut expected = ShardAssignment {
        number_of_shards: 12,
        shard_epochs: HashMap::from([(ShardId::new(3), golem_common::model::ShardEpoch(19))]),
        executor_count: 4,
        revision: Some(golem_common::model::ShardLeaseRevision {
            incarnation: Uuid::parse_str(&incarnation_id()).unwrap(),
            number: 7,
        }),
        expires_at: Some(expiry),
    };
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    service
        .assign_shards(&pod, &assignment(10, 5))
        .await
        .unwrap();
    expected.number_of_shards = 16;
    expected.executor_count = 5;
    expected.revision.as_mut().unwrap().number = 10;
    expected.shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(9)),
        (ShardId::new(2), golem_common::model::ShardEpoch(4)),
    ]);
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    [
        now + Duration::from_secs(60),
        now + Duration::from_secs(20),
        expiry,
    ]
    .into_iter()
    .for_each(|expiry| {
        service.install_registration(&pod, &ack, expiry).unwrap();
        expected.expires_at = Some(expiry);
        assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    });

    let mut newer = ack;
    newer.number_of_shards = 32;
    newer.grant.revision = ShardLeaseRevision(11);
    newer.grant.executor_count = 6;
    newer.grant.shard_epochs.clear();
    service.install_registration(&pod, &newer, now).unwrap();
    expected.number_of_shards = 32;
    expected.executor_count = 6;
    expected.shard_epochs.clear();
    expected.revision.as_mut().unwrap().number = 11;
    expected.expires_at = Some(now);
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    newer.grant.executor_count = 7;
    service.install_registration(&pod, &newer, now).unwrap();
    expected.executor_count = 7;
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
}

#[test]
async fn in_memory_conditions_preserve_assignment_expiry_and_pod_membership() {
    let pod = Pod {
        ip: "127.0.0.1".parse().unwrap(),
        port: 1,
    };
    let other = Pod { port: 2, ..pod };
    let omitted = Pod { port: 3, ..pod };
    let seed = registered(Uuid::parse_str(&incarnation_id()).unwrap());
    let service = InMemoryWorkerExecutorService::new([(pod, seed.clone()), (other, seed.clone())]);
    let mut expected = seed.as_ref().unwrap().clone();
    service
        .set_conditions(
            &pod,
            InMemoryWorkerExecutorConditions {
                health: InMemoryWorkerExecutorHealth::NotServing,
                ..Default::default()
            },
        )
        .unwrap();
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    assert_assignment(&service.current_assignment(&other).unwrap(), &expected);
    assert_health(
        service.health_check(&pod).await,
        InMemoryWorkerExecutorHealth::NotServing,
    );
    service.health_check(&other).await.unwrap();
    service
        .assign_shards(&pod, &assignment(10, 5))
        .await
        .unwrap();
    expected.number_of_shards = 16;
    expected.executor_count = 5;
    expected.revision.as_mut().unwrap().number = 10;
    expected.shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(9)),
        (ShardId::new(2), golem_common::model::ShardEpoch(4)),
    ]);
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    assert_health(
        service.health_check(&pod).await,
        InMemoryWorkerExecutorHealth::NotServing,
    );
    service
        .set_conditions(&pod, InMemoryWorkerExecutorConditions::default())
        .unwrap();
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    service.health_check(&pod).await.unwrap();
    assert_unavailable(service.set_conditions(
        &omitted,
        InMemoryWorkerExecutorConditions {
            health: InMemoryWorkerExecutorHealth::Timeout,
            recovery_index_error: Some(INDEX_OUTAGE.into()),
        },
    ));
    assert!(service.current_assignment(&omitted).is_none());
    assert_unavailable(service.assign_shards(&omitted, &assignment(10, 5)).await);
    assert_unavailable(
        service
            .revoke_shards(&omitted, &BTreeSet::new(), ShardLeaseRevision(10), 5)
            .await,
    );
    match service.health_check(&omitted).await.unwrap_err() {
        HealthCheckError::GrpcError(status) => assert_eq!(status.code(), tonic::Code::Unavailable),
        other => panic!("omitted pod was inserted by conditions: {other:?}"),
    }
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
    assert_assignment(
        &service.current_assignment(&other).unwrap(),
        seed.as_ref().unwrap(),
    );
}

#[test]
async fn in_memory_registration_validates_before_lookup_and_follows_reply_lineage() {
    let pod = Pod {
        ip: "127.0.0.1".parse().unwrap(),
        port: 1,
    };
    let omitted = Pod { port: 2, ..pod };
    let absent = Pod { port: 3, ..pod };
    let (standalone, _) = transition_shard_assignment(
        ShardAssignmentState::default(),
        ShardAssignmentEvent::InstallUnexpiring {
            number_of_shards: 8,
            shard_epochs: &HashMap::new(),
        },
    );
    let service = InMemoryWorkerExecutorService::new([
        (pod, registered(Uuid::from_u128(0xf00))),
        (absent, ShardAssignmentState::default()),
        (Pod { port: 4, ..pod }, standalone),
    ]);
    let expiry = Instant::now() + Duration::from_secs(30);
    let ack = registration();
    [pod, Pod { port: 4, ..pod }]
        .into_iter()
        .for_each(|target| {
            service.install_registration(&target, &ack, expiry).unwrap();
            assert_assignment(
                &service.current_assignment(&target).unwrap(),
                &ShardAssignment {
                    number_of_shards: 12,
                    executor_count: 4,
                    shard_epochs: HashMap::from([(
                        ShardId::new(3),
                        golem_common::model::ShardEpoch(19),
                    )]),
                    revision: Some(golem_common::model::ShardLeaseRevision {
                        incarnation: Uuid::parse_str(&incarnation_id()).unwrap(),
                        number: 7,
                    }),
                    expires_at: Some(expiry),
                },
            );
        });
    let expected = service.current_assignment(&pod).unwrap();
    #[cfg(target_pointer_width = "64")]
    let counts = [0, 4_294_967_296usize, 4_294_967_299];
    #[cfg(not(target_pointer_width = "64"))]
    let counts = [0];
    counts.into_iter().for_each(|count| {
        let mut invalid = ack.clone();
        invalid.grant.executor_count = count;
        [pod, absent, omitted].into_iter().for_each(|target| {
            let detail = if count == 0 {
                "AssignShardsRequest.executor_count must not be 0".to_string()
            } else {
                format!("AssignShardsRequest.executor_count {count} does not fit a u32")
            };
            assert_worker_error(
                service.install_registration(&target, &invalid, expiry + Duration::from_secs(90)),
                WorkerExecutorError::invalid_request(detail),
            );
            assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
            assert!(service.current_assignment(&absent).is_none());
            assert!(service.current_assignment(&omitted).is_none());
        });
    });
    assert_unavailable(service.install_registration(&omitted, &ack, expiry));
    assert!(service.current_assignment(&omitted).is_none());
    service.health_check(&absent).await.unwrap();
    match service.health_check(&omitted).await.unwrap_err() {
        HealthCheckError::GrpcError(status) => assert_eq!(status.code(), tonic::Code::Unavailable),
        other => panic!("omitted pod was inserted: {other:?}"),
    }
    assert_assignment(&service.current_assignment(&pod).unwrap(), &expected);
}
