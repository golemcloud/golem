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
use crate::clients::shard_manager::GrpcShardManagerConfig;
use crate::grpc::client::GrpcClientConfig;
use futures::StreamExt;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::shardmanager::v1 as wire;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::{
    ShardManagerService, ShardManagerServiceServer,
};
use golem_api_grpc::proto::golem::shardmanager::{
    IpAddress, ShardEpochEntry, ShardId as WireShardId, ip_address,
};
use golem_common::model::ShardLeaseRevision;
use golem_schema::proto::golem::common::Empty;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::time::Duration;
use test_r::{test, timeout};
use tokio::sync::Notify;
use tonic::codec::CompressionEncoding;
use tonic::{Request, Response, Status};

const MANAGER: Uuid = Uuid::from_u128(0x5eed);
const EXECUTOR: Uuid = Uuid::from_u128(0xcafe);
const TTL: Duration = Duration::from_secs(60);

fn retries() -> RetryConfig {
    RetryConfig {
        max_attempts: 2,
        min_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        multiplier: 1.0,
        max_jitter_factor: None,
    }
}

fn endpoint() -> ExecutorEndpoint {
    ExecutorEndpoint {
        port: 9123,
        advertised_ip: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9))),
    }
}

fn held() -> BTreeMap<ShardId, ShardEpoch> {
    BTreeMap::from([
        (ShardId::new(2), ShardEpoch(17)),
        (ShardId::new(9), ShardEpoch(23)),
    ])
}

fn wire_epochs() -> Vec<ShardEpochEntry> {
    vec![
        ShardEpochEntry {
            shard_id: Some(WireShardId { value: 2 }),
            epoch: 17,
        },
        ShardEpochEntry {
            shard_id: Some(WireShardId { value: 9 }),
            epoch: 23,
        },
    ]
}

fn grant() -> wire::RegisterSuccess {
    wire::RegisterSuccess {
        number_of_shards: 128,
        shard_epochs: wire_epochs(),
        executor_count: 7,
        lease_ttl: Some(prost_types::Duration {
            seconds: 60,
            nanos: 0,
        }),
        revision: 41,
        incarnation_id: MANAGER.to_string(),
    }
}

fn lease() -> wire::ShardLease {
    wire::ShardLease {
        shard_epochs: wire_epochs(),
        executor_count: 4,
        lease_ttl: Some(prost_types::Duration {
            seconds: 60,
            nanos: 0,
        }),
        revision: 43,
        incarnation_id: MANAGER.to_string(),
    }
}

fn registration(success: wire::RegisterSuccess) -> RegisterResponse {
    RegisterResponse {
        result: Some(register_response::Result::Success(success)),
    }
}

fn renewal(success: wire::ShardLease) -> RenewShardLeaseResponse {
    RenewShardLeaseResponse {
        result: Some(renew_shard_lease_response::Result::Success(success)),
    }
}

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}

#[derive(Default)]
struct Script {
    registrations: Mutex<VecDeque<Result<RegisterResponse, Status>>>,
    renewals: Mutex<VecDeque<Result<RenewShardLeaseResponse, Status>>>,
    deregistrations: Mutex<VecDeque<Result<DeregisterResponse, Status>>>,
    register_requests: Mutex<Vec<(Instant, RegisterRequest)>>,
    renew_requests: Mutex<Vec<RenewShardLeaseRequest>>,
    deregister_requests: Mutex<Vec<DeregisterRequest>>,
    registration_gates: [Option<Arc<Gate>>; 2],
}

// Record before constructing the future. This also detects a memory adapter that anchors the
// TTL after invoking its callback but before polling the returned future.
fn register_call(
    script: Arc<Script>,
    request: RegisterRequest,
) -> BoxFuture<'static, Result<RegisterResponse, Status>> {
    let attempt = {
        let mut requests = script.register_requests.lock().unwrap();
        requests.push((Instant::now(), request));
        requests.len()
    };
    let gate = script
        .registration_gates
        .get(attempt - 1)
        .cloned()
        .flatten();
    let response = script.registrations.lock().unwrap().pop_front();
    Box::pin(async move {
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        response
            .unwrap_or_else(|| Err(Status::failed_precondition("registration script exhausted")))
    })
}

fn renew_call(
    script: &Script,
    request: RenewShardLeaseRequest,
) -> Result<RenewShardLeaseResponse, Status> {
    script.renew_requests.lock().unwrap().push(request);
    script
        .renewals
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Err(Status::failed_precondition("renewal script exhausted")))
}

fn deregister_call(
    script: &Script,
    request: DeregisterRequest,
) -> Result<DeregisterResponse, Status> {
    script.deregister_requests.lock().unwrap().push(request);
    script
        .deregistrations
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| {
            Err(Status::failed_precondition(
                "deregistration script exhausted",
            ))
        })
}

#[derive(Clone)]
struct Server(Arc<Script>);

#[tonic::async_trait]
impl ShardManagerService for Server {
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        register_call(self.0.clone(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn renew_shard_lease(
        &self,
        request: Request<RenewShardLeaseRequest>,
    ) -> Result<Response<RenewShardLeaseResponse>, Status> {
        renew_call(&self.0, request.into_inner()).map(Response::new)
    }

    async fn deregister(
        &self,
        request: Request<DeregisterRequest>,
    ) -> Result<Response<DeregisterResponse>, Status> {
        deregister_call(&self.0, request.into_inner()).map(Response::new)
    }

    async fn get_routing_table(
        &self,
        _: Request<wire::GetRoutingTableRequest>,
    ) -> Result<Response<wire::GetRoutingTableResponse>, Status> {
        Err(Status::unimplemented("routing"))
    }

    async fn acquire_quota_lease(
        &self,
        _: Request<wire::AcquireQuotaLeaseRequest>,
    ) -> Result<Response<wire::AcquireQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("quota"))
    }

    async fn renew_quota_lease(
        &self,
        _: Request<wire::RenewQuotaLeaseRequest>,
    ) -> Result<Response<wire::RenewQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("quota"))
    }

    async fn batch_renew_quota_leases(
        &self,
        _: Request<wire::BatchRenewQuotaLeasesRequest>,
    ) -> Result<Response<wire::BatchRenewQuotaLeasesResponse>, Status> {
        Err(Status::unimplemented("quota"))
    }

    async fn release_quota_lease(
        &self,
        _: Request<wire::ReleaseQuotaLeaseRequest>,
    ) -> Result<Response<wire::ReleaseQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("quota"))
    }
}

#[derive(Clone, Copy)]
enum Adapter {
    Memory,
    Grpc,
}

async fn client(
    adapter: Adapter,
    script: Arc<Script>,
) -> (
    Arc<dyn ShardLeaseClient>,
    Option<tokio::task::JoinHandle<()>>,
) {
    match adapter {
        Adapter::Memory => {
            let register = script.clone();
            let renew = script.clone();
            let client = InMemoryShardLeaseClient::new(
                retries(),
                move |request| {
                    let response = register_call(register.clone(), request);
                    async move { response.await.map_err(Into::into) }
                },
                move |request| {
                    let response = renew_call(&renew, request);
                    async move { response.map_err(Into::into) }
                },
                move |request| {
                    let response = deregister_call(&script, request);
                    async move { response.map_err(Into::into) }
                },
            );
            (Arc::new(client), None)
        }
        Adapter::Grpc => {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(
                        ShardManagerServiceServer::new(Server(script))
                            .accept_compressed(CompressionEncoding::Gzip)
                            .send_compressed(CompressionEncoding::Gzip),
                    )
                    .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                    .await
                    .unwrap();
            });
            let client = GrpcShardManager::new(&GrpcShardManagerConfig {
                host: "127.0.0.1".to_string(),
                port,
                client_config: GrpcClientConfig {
                    retries_on_unavailable: RetryConfig {
                        max_attempts: 1,
                        ..retries()
                    },
                    ..GrpcClientConfig::default()
                },
                retries: retries(),
            });
            (Arc::new(client), Some(server))
        }
    }
}

async fn stop(server: Option<tokio::task::JoinHandle<()>>) {
    if let Some(server) = server {
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}

fn assert_register_requests(script: &Script, count: usize) {
    let requests = std::mem::take(&mut *script.register_requests.lock().unwrap());
    assert_eq!(requests.len(), count);
    requests.into_iter().for_each(|(_, request)| {
        assert_eq!(
            request,
            RegisterRequest {
                endpoint: Some(wire::ExecutorEndpoint {
                    port: 9123,
                    advertised_ip: Some(IpAddress {
                        kind: Some(ip_address::Kind::Ipv4(0xc0000209))
                    }),
                }),
                pod_name: Some("executor-pod".to_string()),
                executor_id: "00000000-0000-0000-0000-00000000cafe".to_string(),
            }
        );
    });
    assert!(script.registrations.lock().unwrap().is_empty());
}

fn assert_renew_request(script: &Script) {
    assert_eq!(
        std::mem::take(&mut *script.renew_requests.lock().unwrap()),
        vec![RenewShardLeaseRequest {
            executor_id: "00000000-0000-0000-0000-00000000cafe".to_string(),
            shard_epochs: wire_epochs(),
        }]
    );
    assert!(script.renewals.lock().unwrap().is_empty());
}

fn assert_deregister_request(script: &Script) {
    assert_eq!(
        std::mem::take(&mut *script.deregister_requests.lock().unwrap()),
        vec![DeregisterRequest {
            executor_id: "00000000-0000-0000-0000-00000000cafe".to_string(),
            shard_epochs: wire_epochs(),
        }]
    );
    assert!(script.deregistrations.lock().unwrap().is_empty());
}

fn assert_lease_fields(
    lease: &ShardLease,
    count: usize,
    revision: u64,
    before: Instant,
    after: Instant,
) {
    assert_eq!(lease.executor_count.get(), count);
    assert_eq!(lease.shard_epochs, held());
    assert_eq!(
        lease.revision,
        ShardLeaseRevision {
            incarnation: MANAGER,
            number: revision
        }
    );
    assert!(lease.expires_at >= before + TTL);
    assert!(lease.expires_at <= after + TTL);
}

async fn contract(adapter: Adapter) {
    let script = Arc::new(Script::default());
    let (client, server) = client(adapter, script.clone()).await;

    // A malformed registration is retried outside the router, including the decode step.
    script.registrations.lock().unwrap().extend([
        Ok(registration(wire::RegisterSuccess {
            executor_count: 0,
            ..grant()
        })),
        Ok(registration(grant())),
    ]);
    let before = Instant::now();
    let registered = client
        .register(endpoint(), Some("executor-pod".to_string()), EXECUTOR)
        .await
        .unwrap();
    assert_eq!(registered.number_of_shards, 128);
    assert_lease_fields(&registered.lease, 7, 41, before, Instant::now());
    assert_register_requests(&script, 2);

    script
        .renewals
        .lock()
        .unwrap()
        .push_back(Ok(renewal(lease())));
    let before = Instant::now();
    let renewed = client.renew_shard_lease(EXECUTOR, held()).await.unwrap();
    assert_lease_fields(&renewed, 4, 43, before, Instant::now());
    assert_renew_request(&script);

    // Non-UUID incarnation errors contain UUID parser detail. Check the error arm and the exact
    // field prefix separately, without reproducing the parser in the expected result.
    script
        .registrations
        .lock()
        .unwrap()
        .extend(std::iter::repeat_n(
            Ok(registration(wire::RegisterSuccess {
                incarnation_id: String::new(),
                ..grant()
            })),
            2,
        ));
    let error = client
        .register(endpoint(), Some("executor-pod".to_string()), EXECUTOR)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ShardManagerError::ConversionError(message) if message.starts_with("RegisterSuccess.incarnation_id \"\" is not a UUID:"))
    );
    assert_register_requests(&script, 2);

    let invalid_registrations = [
        (
            RegisterResponse { result: None },
            "Internal client error: empty response",
        ),
        (
            registration(wire::RegisterSuccess {
                executor_count: 0,
                ..grant()
            }),
            "Conversion error: RegisterSuccess.executor_count must not be 0",
        ),
        (
            registration(wire::RegisterSuccess {
                lease_ttl: None,
                ..grant()
            }),
            "Conversion error: lease_ttl is required",
        ),
        (
            registration(wire::RegisterSuccess {
                lease_ttl: Some(prost_types::Duration {
                    seconds: -1,
                    nanos: 0,
                }),
                ..grant()
            }),
            "Conversion error: lease_ttl is negative",
        ),
        (
            registration(wire::RegisterSuccess {
                lease_ttl: Some(prost_types::Duration {
                    seconds: 1,
                    nanos: 1_000_000_000,
                }),
                ..grant()
            }),
            "Conversion error: lease_ttl has out-of-range nanoseconds",
        ),
        (
            registration(wire::RegisterSuccess {
                shard_epochs: vec![ShardEpochEntry {
                    shard_id: None,
                    epoch: 17,
                }],
                ..grant()
            }),
            "Conversion error: ShardEpochEntry.shard_id missing",
        ),
        (
            RegisterResponse {
                result: Some(register_response::Result::Failure(
                    wire::ShardManagerError { error: None },
                )),
            },
            "Internal client error: Missing error field",
        ),
        (
            RegisterResponse {
                result: Some(register_response::Result::Failure(
                    wire::ShardManagerError {
                        error: Some(wire::shard_manager_error::Error::InvalidRequest(
                            ErrorBody {
                                error: "bad endpoint".to_string(),
                                code: String::new(),
                            },
                        )),
                    },
                )),
            },
            "Invalid request: bad endpoint",
        ),
        (
            RegisterResponse {
                result: Some(register_response::Result::Failure(
                    wire::ShardManagerError {
                        error: Some(wire::shard_manager_error::Error::Timeout(ErrorBody {
                            error: "late".to_string(),
                            code: String::new(),
                        })),
                    },
                )),
            },
            "Timeout: late",
        ),
        (
            RegisterResponse {
                result: Some(register_response::Result::Failure(
                    wire::ShardManagerError {
                        error: Some(wire::shard_manager_error::Error::Unknown(ErrorBody {
                            error: "storage".to_string(),
                            code: String::new(),
                        })),
                    },
                )),
            },
            "Internal server error: storage",
        ),
    ];
    futures::stream::iter(invalid_registrations)
        .for_each(|(response, expected)| {
            let client = client.clone();
            let script = script.clone();
            async move {
                script
                    .registrations
                    .lock()
                    .unwrap()
                    .extend(std::iter::repeat_n(Ok(response), 2));
                let error = client
                    .register(endpoint(), Some("executor-pod".to_string()), EXECUTOR)
                    .await
                    .unwrap_err();
                assert_eq!(error.to_string(), expected);
                assert_register_requests(&script, 2);
            }
        })
        .await;

    script
        .renewals
        .lock()
        .unwrap()
        .push_back(Ok(renewal(wire::ShardLease {
            incarnation_id: "not-a-uuid".to_string(),
            ..lease()
        })));
    let error = client
        .renew_shard_lease(EXECUTOR, held())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ShardLeaseError::ConversionError(message) if message.starts_with("ShardLease.incarnation_id \"not-a-uuid\" is not a UUID:"))
    );
    assert_renew_request(&script);

    let invalid_renewals = [
        (
            RenewShardLeaseResponse { result: None },
            "Internal client error: empty response",
        ),
        (
            renewal(wire::ShardLease {
                executor_count: 0,
                ..lease()
            }),
            "Conversion error: ShardLease.executor_count must not be 0",
        ),
        (
            renewal(wire::ShardLease {
                lease_ttl: None,
                ..lease()
            }),
            "Conversion error: lease_ttl is required",
        ),
        (
            renewal(wire::ShardLease {
                lease_ttl: Some(prost_types::Duration {
                    seconds: -1,
                    nanos: 0,
                }),
                ..lease()
            }),
            "Conversion error: lease_ttl is negative",
        ),
        (
            renewal(wire::ShardLease {
                lease_ttl: Some(prost_types::Duration {
                    seconds: 1,
                    nanos: -1,
                }),
                ..lease()
            }),
            "Conversion error: lease_ttl has out-of-range nanoseconds",
        ),
        (
            renewal(wire::ShardLease {
                shard_epochs: vec![ShardEpochEntry {
                    shard_id: None,
                    epoch: 17,
                }],
                ..lease()
            }),
            "Conversion error: ShardEpochEntry.shard_id missing",
        ),
    ];
    futures::stream::iter(invalid_renewals)
        .for_each(|(response, expected)| {
            let client = client.clone();
            let script = script.clone();
            async move {
                script.renewals.lock().unwrap().push_back(Ok(response));
                let error = client
                    .renew_shard_lease(EXECUTOR, held())
                    .await
                    .unwrap_err();
                assert_eq!(error.to_string(), expected);
                assert_renew_request(&script);
            }
        })
        .await;

    let failures = [
        (
            wire::ShardLeaseError { error: None },
            "Internal client error: Missing error field",
        ),
        (
            wire::ShardLeaseError {
                error: Some(wire::shard_lease_error::Error::LeaseNotFound(ErrorBody {
                    error: "unknown executor".to_string(),
                    code: String::new(),
                })),
            },
            "Shard lease not found: unknown executor",
        ),
        (
            wire::ShardLeaseError {
                error: Some(wire::shard_lease_error::Error::Internal(ErrorBody {
                    error: "lease store".to_string(),
                    code: String::new(),
                })),
            },
            "Internal server error: lease store",
        ),
    ];
    futures::stream::iter(failures)
        .for_each(|(failure, expected)| {
            let client = client.clone();
            let script = script.clone();
            async move {
                script
                    .renewals
                    .lock()
                    .unwrap()
                    .push_back(Ok(RenewShardLeaseResponse {
                        result: Some(renew_shard_lease_response::Result::Failure(failure.clone())),
                    }));
                script
                    .deregistrations
                    .lock()
                    .unwrap()
                    .push_back(Ok(DeregisterResponse {
                        result: Some(deregister_response::Result::Failure(failure)),
                    }));
                assert_eq!(
                    client
                        .renew_shard_lease(EXECUTOR, held())
                        .await
                        .unwrap_err()
                        .to_string(),
                    expected
                );
                assert_eq!(
                    client
                        .deregister(EXECUTOR, held())
                        .await
                        .unwrap_err()
                        .to_string(),
                    expected
                );
                assert_renew_request(&script);
                assert_deregister_request(&script);
            }
        })
        .await;

    script
        .deregistrations
        .lock()
        .unwrap()
        .push_back(Ok(DeregisterResponse { result: None }));
    assert_eq!(
        client
            .deregister(EXECUTOR, held())
            .await
            .unwrap_err()
            .to_string(),
        "Internal client error: empty response"
    );
    assert_deregister_request(&script);
    script
        .deregistrations
        .lock()
        .unwrap()
        .push_back(Ok(DeregisterResponse {
            result: Some(deregister_response::Result::Success(Empty {})),
        }));
    client.deregister(EXECUTOR, held()).await.unwrap();
    assert_deregister_request(&script);

    // This status is a final routed failure. It neither redirects nor requests a replica retry.
    let status = Status::permission_denied("blocked");
    let expected = format!("Internal client error: Connection error: {status}");
    script
        .registrations
        .lock()
        .unwrap()
        .extend(std::iter::repeat_n(Err(status.clone()), 2));
    script
        .renewals
        .lock()
        .unwrap()
        .push_back(Err(status.clone()));
    script
        .deregistrations
        .lock()
        .unwrap()
        .push_back(Err(status));
    assert_eq!(
        client
            .register(endpoint(), Some("executor-pod".to_string()), EXECUTOR)
            .await
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(
        client
            .renew_shard_lease(EXECUTOR, held())
            .await
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(
        client
            .deregister(EXECUTOR, held())
            .await
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_register_requests(&script, 2);
    assert_renew_request(&script);
    assert_deregister_request(&script);

    // With one routed attempt, UNAVAILABLE is final too. An accidental retried router call
    // sends an extra request, even when its next response is a different failure.
    let status = Status::unavailable("lease manager unavailable");
    let expected = format!("Internal client error: Connection error: {status}");
    script
        .renewals
        .lock()
        .unwrap()
        .push_back(Err(status.clone()));
    script
        .deregistrations
        .lock()
        .unwrap()
        .push_back(Err(status));
    assert_eq!(
        client
            .renew_shard_lease(EXECUTOR, held())
            .await
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(
        client
            .deregister(EXECUTOR, held())
            .await
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_renew_request(&script);
    assert_deregister_request(&script);
    stop(server).await;

    second_attempt_anchor(adapter).await;
}

async fn second_attempt_anchor(adapter: Adapter) {
    let first = Arc::new(Gate::default());
    let second = Arc::new(Gate::default());
    let script = Arc::new(Script {
        registration_gates: [Some(first.clone()), Some(second.clone())],
        ..Script::default()
    });
    script.registrations.lock().unwrap().extend([
        Ok(registration(wire::RegisterSuccess {
            executor_count: 0,
            ..grant()
        })),
        Ok(registration(grant())),
    ]);
    let (client, server) = client(adapter, script.clone()).await;
    let registered = tokio::spawn(async move {
        client
            .register(endpoint(), Some("executor-pod".to_string()), EXECUTOR)
            .await
    });
    first.entered.notified().await;
    let first_released = Instant::now();
    first.release.notify_one();
    second.entered.notified().await;
    let callback_invoked = script.register_requests.lock().unwrap()[1].0;
    let second_released = Instant::now();
    second.release.notify_one();
    let lease = registered.await.unwrap().unwrap().lease;
    assert_eq!(lease.executor_count.get(), 7);
    assert!(
        lease.expires_at >= first_released + TTL,
        "the successful attempt needs its own anchor"
    );
    assert!(
        lease.expires_at <= callback_invoked + TTL,
        "the anchor must precede callback invocation"
    );
    assert!(
        lease.expires_at < second_released + TTL,
        "waiting for the response must consume TTL"
    );
    assert_register_requests(&script, 2);
    stop(server).await;
}

#[test]
#[timeout("30s")]
async fn in_memory_shard_lease_client_contract() {
    contract(Adapter::Memory).await;
}

#[test]
#[timeout("30s")]
async fn grpc_shard_lease_client_contract() {
    contract(Adapter::Grpc).await;
}

#[test]
fn routed_errors_keep_their_raw_payload_when_mapped_to_shard_lease_errors() {
    [
        (
            ShardManagerError::InvalidRequest("request".to_string()),
            "Internal client error: request",
        ),
        (
            ShardManagerError::Timeout("late".to_string()),
            "Internal client error: late",
        ),
        (
            ShardManagerError::ConversionError("wire".to_string()),
            "Conversion error: wire",
        ),
        (
            ShardManagerError::InternalServerError("store".to_string()),
            "Internal server error: store",
        ),
        (
            ShardManagerError::InternalClientError("connection".to_string()),
            "Internal client error: connection",
        ),
    ]
    .into_iter()
    .for_each(|(error, expected)| {
        assert_eq!(ShardLeaseError::from(error).to_string(), expected);
    });
}
