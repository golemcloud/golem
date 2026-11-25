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
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, AssignShardsResponse, RevokeShardsRequest, RevokeShardsResponse,
    assign_shards_response, revoke_shards_response,
};
use golem_common::model::protobuf::shard_epochs_from_proto;
use golem_common::model::sharding::{
    ShardAssignmentEvent, ShardAssignmentState, transition_shard_assignment,
};
use golem_common::model::{RetryConfig, ShardAssignment, ShardDeliveryOutcome};
use golem_schema::proto::golem::common::Empty;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::{Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::body::Body;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::Service;
use tonic::server::{NamedService, UnaryService};
use tonic::{Request, Status};
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
    last_revoke: Arc<Mutex<Option<RevokeShardsRequest>>>,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<tokio::task::JoinHandle<Result<(), tonic::transport::Error>>>,
}

impl Fixture {
    async fn new(adapter: Adapter, state: ShardAssignmentState) -> Self {
        let last_revoke = Arc::new(Mutex::new(None));
        match adapter {
            Adapter::Memory => {
                let pod = Pod {
                    ip: "127.0.0.1".parse().unwrap(),
                    port: 1,
                };
                let service = Arc::new(InMemoryWorkerExecutorService::new([(pod, state)]));
                let observed = service.clone();
                Self {
                    service,
                    pod,
                    snapshot: Box::new(move || observed.current_assignment(&pod)),
                    last_revoke,
                    shutdown: None,
                    server: None,
                }
            }
            Adapter::Grpc => {
                let state = Arc::new(RwLock::new(state));
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let pod = Pod {
                    ip: address.ip(),
                    port: address.port(),
                };
                let (shutdown, stopped) = oneshot::channel();
                let (health, health_service) = tonic_health::server::health_reporter();
                health
                    .set_service_status("", tonic_health::ServingStatus::Serving)
                    .await;
                let server = tokio::spawn(
                    tonic::transport::Server::builder()
                        .add_service(ExecutorServer {
                            state: state.clone(),
                            last_revoke: last_revoke.clone(),
                        })
                        .add_service(health_service)
                        .serve_with_incoming_shutdown(
                            tokio_stream::wrappers::TcpListenerStream::new(listener),
                            async move {
                                let _ = stopped.await;
                            },
                        ),
                );
                Self {
                    service: Arc::new(WorkerExecutorServiceDefault::new(
                        WorkerExecutorServiceConfig {
                            retries: RetryConfig::no_retries(),
                            ..Default::default()
                        },
                    )),
                    pod,
                    snapshot: Box::new(move || state.read().unwrap().as_ref().cloned()),
                    last_revoke,
                    shutdown: Some(shutdown),
                    server: Some(server),
                }
            }
        }
    }

    async fn finish(self) {
        if let Some(shutdown) = self.shutdown {
            let _ = shutdown.send(());
        }
        // Drop the pooled client before the server waits for its connections to close.
        drop(self.service);
        if let Some(server) = self.server {
            server.await.unwrap().unwrap();
        }
    }
}

fn registered(incarnation: Uuid) -> ShardAssignmentState {
    let shard_epochs = HashMap::from([
        (ShardId::new(0), golem_common::model::ShardEpoch(7)),
        (ShardId::new(1), golem_common::model::ShardEpoch(8)),
    ]);
    transition_shard_assignment(
        ShardAssignmentState::default(),
        ShardAssignmentEvent::Register {
            number_of_shards: 8,
            shard_epochs: &shard_epochs,
            expires_at: Some(Instant::now() + Duration::from_secs(60)),
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
    if let Adapter::Grpc = adapter {
        let request = fixture.last_revoke.lock().unwrap().clone().unwrap();
        assert_eq!(request.executor_count, 3);
        assert_eq!(request.revision, 11);
        assert_eq!(request.incarnation_id, incarnation_id());
        assert_eq!(
            request
                .shard_ids
                .into_iter()
                .map(ShardId::from)
                .collect::<BTreeSet<_>>(),
            removed
        );
    }

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
    fixture.finish().await;

    let fixture = Fixture::new(adapter, ShardAssignmentState::default()).await;
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
            .revoke_shards(&fixture.pod, &removed, ShardLeaseRevision(1), 3)
            .await,
        WorkerExecutorError::ShardingNotReady,
    );
    assert!((fixture.snapshot)().is_none());
    fixture.finish().await;

    let other_manager = Uuid::from_u128(0xf00);
    assert_ne!(other_manager, incarnation);
    let fixture = Fixture::new(adapter, registered(other_manager)).await;
    let expected = (fixture.snapshot)().unwrap();
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

// Only the two shard RPCs are served. The real client still builds, compresses and sends each
// request and decodes each response. The receiver applies the common executor transition.
#[derive(Clone)]
struct ExecutorServer {
    state: Arc<RwLock<ShardAssignmentState>>,
    last_revoke: Arc<Mutex<Option<RevokeShardsRequest>>>,
}

impl NamedService for ExecutorServer {
    const NAME: &'static str = "golem.workerexecutor.v1.WorkerExecutor";
}

impl Service<http::Request<Body>> for ExecutorServer {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let server = self.clone();
        let is_assignment = request.uri().path().ends_with("/AssignShards");
        Box::pin(async move {
            if is_assignment {
                let mut grpc = tonic::server::Grpc::new(Protobuf::<
                    AssignShardsResponse,
                    AssignShardsRequest,
                >::default())
                .accept_compressed(CompressionEncoding::Gzip)
                .send_compressed(CompressionEncoding::Gzip);
                Ok(grpc.unary(server, request).await)
            } else {
                let mut grpc = tonic::server::Grpc::new(Protobuf::<
                    RevokeShardsResponse,
                    RevokeShardsRequest,
                >::default())
                .accept_compressed(CompressionEncoding::Gzip)
                .send_compressed(CompressionEncoding::Gzip);
                Ok(grpc.unary(server, request).await)
            }
        })
    }
}

impl UnaryService<AssignShardsRequest> for ExecutorServer {
    type Response = AssignShardsResponse;
    type Future = BoxFuture<'static, Result<Response<Self::Response>, Status>>;

    fn call(&mut self, request: Request<AssignShardsRequest>) -> Self::Future {
        let result = receive_assignment(&mut self.state.write().unwrap(), request.into_inner());
        Box::pin(async move {
            Ok(Response::new(AssignShardsResponse {
                result: Some(match result {
                    Ok(()) => assign_shards_response::Result::Success(Empty {}),
                    Err(error) => assign_shards_response::Result::Failure(error.into()),
                }),
            }))
        })
    }
}

impl UnaryService<RevokeShardsRequest> for ExecutorServer {
    type Response = RevokeShardsResponse;
    type Future = BoxFuture<'static, Result<Response<Self::Response>, Status>>;

    fn call(&mut self, request: Request<RevokeShardsRequest>) -> Self::Future {
        let request = request.into_inner();
        *self.last_revoke.lock().unwrap() = Some(request.clone());
        let result = receive_revoke(&mut self.state.write().unwrap(), request);
        Box::pin(async move {
            Ok(Response::new(RevokeShardsResponse {
                result: Some(match result {
                    Ok(()) => revoke_shards_response::Result::Success(Empty {}),
                    Err(error) => revoke_shards_response::Result::Failure(error.into()),
                }),
            }))
        })
    }
}

fn receive_assignment(
    state: &mut ShardAssignmentState,
    request: AssignShardsRequest,
) -> Result<(), WorkerExecutorError> {
    let shard_epochs = shard_epochs_from_proto(request.shard_epochs)
        .map_err(WorkerExecutorError::invalid_request)?;
    if request.number_of_shards == 0 {
        return Err(WorkerExecutorError::invalid_request(
            "AssignShardsRequest.number_of_shards must not be 0",
        ));
    }
    let executor_count = NonZeroUsize::new(request.executor_count as usize).ok_or_else(|| {
        WorkerExecutorError::invalid_request("AssignShardsRequest.executor_count must not be 0")
    })?;
    let revision = golem_common::model::ShardLeaseRevision::from_wire(
        &request.incarnation_id,
        request.revision,
    )
    .map_err(|error| {
        WorkerExecutorError::invalid_request(format!("AssignShardsRequest.{error}"))
    })?;
    receive(
        state,
        ShardAssignmentEvent::Assign {
            number_of_shards: request.number_of_shards as usize,
            shard_epochs: &shard_epochs,
            revision,
            executor_count,
        },
    )
}

fn receive_revoke(
    state: &mut ShardAssignmentState,
    request: RevokeShardsRequest,
) -> Result<(), WorkerExecutorError> {
    let executor_count = NonZeroUsize::new(request.executor_count as usize).ok_or_else(|| {
        WorkerExecutorError::invalid_request("RevokeShardsRequest.executor_count must not be 0")
    })?;
    let revision = golem_common::model::ShardLeaseRevision::from_wire(
        &request.incarnation_id,
        request.revision,
    )
    .map_err(|error| {
        WorkerExecutorError::invalid_request(format!("RevokeShardsRequest.{error}"))
    })?;
    let shard_ids: HashSet<_> = request.shard_ids.into_iter().map(ShardId::from).collect();
    receive(
        state,
        ShardAssignmentEvent::Revoke {
            shard_ids: &shard_ids,
            revision,
            executor_count,
        },
    )
}

fn receive(
    state: &mut ShardAssignmentState,
    event: ShardAssignmentEvent<'_>,
) -> Result<(), WorkerExecutorError> {
    let (next, outcome) = transition_shard_assignment(std::mem::take(state), event);
    *state = next;
    outcome
        .map(|_: ShardDeliveryOutcome| ())
        .ok_or(WorkerExecutorError::ShardingNotReady)
}

// The package already has prost and tonic. A small codec avoids adding a dependency just for
// the two-method test server.
struct Protobuf<E, D>(PhantomData<(E, D)>);

impl<E, D> Default for Protobuf<E, D> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E, D> Codec for Protobuf<E, D>
where
    E: Message + Send + 'static,
    D: Message + Default + Send + 'static,
{
    type Encode = E;
    type Decode = D;
    type Encoder = Self;
    type Decoder = Self;

    fn encoder(&mut self) -> Self::Encoder {
        Self::default()
    }

    fn decoder(&mut self) -> Self::Decoder {
        Self::default()
    }
}

impl<E: Message, D> Encoder for Protobuf<E, D> {
    type Item = E;
    type Error = Status;

    fn encode(&mut self, item: E, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst)
            .map_err(|error| Status::internal(error.to_string()))
    }
}

impl<E, D: Message + Default> Decoder for Protobuf<E, D> {
    type Item = D;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<D>, Status> {
        D::decode(src)
            .map(Some)
            .map_err(|error| Status::internal(error.to_string()))
    }
}
