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
use crate::durable_host::DurableHttpHooks;
use crate::durable_host::durability::DurableExecutionState;
use crate::durable_host::http::transport::{
    CapturedRequestBody, InMemoryHttpTransport, RequestBodyTiming, ScriptedHttpExchange,
};
use crate::durable_host::p3::http::send::driver::{DriverExit, drive_send};
use crate::durable_host::p3::http::test_support::{FrameTestOplog, TestHttpCtx};
use crate::services::oplog::OplogError;
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::{Predicate, PredicateValue, RetryPolicy, RetryPolicyState, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use http_body_util::{Empty, Full, StreamBody};
use std::collections::HashMap;
use std::time::Duration;
use test_r::{test, timeout};
use wasmtime_wasi_http::WasiHttpView;

fn physical_request(
    guest_headers: &HashMap<String, Vec<Vec<u8>>>,
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    recording_enabled: bool,
) -> PhysicalSendRequest<crate::workerctx::default::Context> {
    let mut store = wasmtime::Store::new(&wasmtime::Engine::default(), TestHttpCtx::default());
    let headers = guest_headers
        .iter()
        .flat_map(|(name, values)| {
            values.iter().map(move |value| {
                (
                    http::HeaderName::try_from(name.as_str()).unwrap(),
                    http::HeaderValue::from_bytes(value).unwrap(),
                )
            })
        })
        .collect::<HeaderMap>();
    let fields =
        wasmtime_wasi_http::FieldMap::new_immutable(store.data_mut().http().hooks, headers);
    let (request, transmission) = wasmtime_wasi_http::p3::Request::new(
        http::Method::POST,
        Some(http::uri::Scheme::HTTP),
        Some(http::uri::Authority::from_static("example.com")),
        Some(http::uri::PathAndQuery::from_static("/charge/93")),
        fields,
        None,
        body,
    );
    let req = store.data_mut().table.push(request).unwrap();
    let position = crate::durable_host::IdempotencyKeyPosition {
        parent: golem_common::model::InvocationId(
            Uuid::parse_str("7b536e18-3ac7-4cf1-9931-b552f4ef728d").unwrap(),
        ),
        logical: Some(OplogIndex::from_u64(317)),
    };
    let managed = golem_outgoing_http_headers(
        store.data().table.get(&req).unwrap(),
        vec![("traceparent".to_string(), "trace-charge-93".to_string())],
        true,
        OplogIndex::from_u64(941),
        &position,
    );
    apply_headers_to_request_resource(
        store.data_mut().http(),
        &Resource::new_borrow(req.rep()),
        &managed,
    )
    .unwrap();
    let request = store.data_mut().table.delete(req).unwrap();
    let (request, options) = request.into_http(&mut store, async { Ok(()) }).unwrap();
    assert!(matches!(transmission.now_or_never(), Some(Ok(()))));
    let (head, body) = request.into_parts();
    let mut hooks = DurableHttpHooks {
        connection_pool: None,
        is_replay: Arc::new(AtomicBool::new(false)),
    };
    let body = body
        .map_err(move |error| hooks.transport_error_to_p3(&error))
        .boxed_unsync();

    PhysicalSendRequest {
        pool: None,
        hooks: DurableHttpHooks {
            connection_pool: None,
            is_replay: Arc::new(AtomicBool::new(false)),
        },
        method: head.method,
        uri: head.uri,
        version: head.version,
        headers: head.headers,
        body: DurableRequestBody::new(
            body,
            FrameTestOplog::new(),
            OplogIndex::from_u64(941),
            recording_enabled,
            Default::default(),
        ),
        options: options.map(|options| *options),
        final_transmission_tx: oneshot::channel().0,
        _phantom: PhantomData,
    }
}

fn response(status: u16, timing: RequestBodyTiming) -> ScriptedHttpExchange {
    ScriptedHttpExchange::response(
        http::Response::builder()
            .status(status)
            .body(
                Full::new(Bytes::from_static(b"charge-accepted"))
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .unwrap(),
        timing,
        Box::new(async { Ok(()) }),
    )
}

#[test]
#[timeout("10s")]
async fn physical_attempt_preserves_guest_key() {
    let physical = physical_request(
        &HashMap::from([(
            "idempotency-key".to_string(),
            vec![b"business-charge-93-revision-7".to_vec()],
        )]),
        Full::new(Bytes::from_static(b"charge=93&currency=EUR"))
            .map_err(|never| match never {})
            .boxed_unsync(),
        true,
    );
    let transport = InMemoryHttpTransport::new([response(201, RequestBodyTiming::BeforeResponse)]);
    let (response, io, _invalidate) = physical.attempt(&transport).await.unwrap();
    assert_eq!(response.status(), 201);
    Box::into_pin(io).await.unwrap();
    assert_eq!(transport.invalidation_count(), 0);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 1);
    let request = &captures[0].request;
    assert_eq!(request.method(), http::Method::POST);
    assert_eq!(request.uri(), "http://example.com/charge/93");
    assert_eq!(
        request.headers()["idempotency-key"],
        "business-charge-93-revision-7"
    );
    assert_eq!(
        request.headers().get_all("idempotency-key").iter().count(),
        1
    );
    assert_eq!(request.headers()["traceparent"], "trace-charge-93");
    let CapturedRequestBody::Complete(body) = request.body().as_ref() else {
        panic!("the transport did not read the complete request body");
    };
    assert_eq!(body.bytes, b"charge=93&currency=EUR".as_slice());
}

fn status_predicate() -> Predicate {
    Predicate::PropEq {
        property: "status-code".to_string(),
        value: PredicateValue::Integer(503),
    }
}

#[test]
#[timeout("10s")]
async fn retry_driver_status_retries_preserve_key_and_body() {
    let mut physical = physical_request(&HashMap::new(), request_body(), true);
    let options = Some(wasmtime_wasi_http::RequestOptions {
        connect_timeout: Some(Duration::from_secs(7)),
        first_byte_timeout: Some(Duration::from_secs(11)),
        between_bytes_timeout: Some(Duration::from_secs(13)),
    });
    physical.options = options;
    let transport = InMemoryHttpTransport::new([
        response(503, RequestBodyTiming::BeforeResponse),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(status_predicate(), 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async {
            assert_eq!(transport.invalidation_count(), 1);
            Ok(())
        },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("the status retry failed")
    };
    assert_eq!(result.response.status(), 201);
    assert_eq!(host.recorded, 1);
    assert_eq!(transport.invalidation_count(), 1);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 2);
    captures.iter().for_each(|capture| {
        assert_eq!(capture.options, options);
        assert_eq!(
            capture.request.headers()["idempotency-key"],
            "65a19192-a492-51cb-bd6f-2b403992359a"
        );
        assert_eq!(
            capture
                .request
                .headers()
                .get_all("idempotency-key")
                .iter()
                .count(),
            1
        );
        let CapturedRequestBody::Complete(body) = capture.request.body().as_ref() else {
            panic!("a status retry did not transmit the complete body");
        };
        assert_eq!(body.bytes, b"charge=93&currency=EUR".as_slice());
    });
    let (CapturedRequestBody::Complete(first), CapturedRequestBody::Complete(second)) = (
        captures[0].request.body().as_ref(),
        captures[1].request.body().as_ref(),
    ) else {
        unreachable!()
    };
    assert_eq!(first.bytes, second.bytes);
}

#[test]
#[timeout("10s")]
async fn retry_driver_status_exhaustion_returns_latest_response() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        response(503, RequestBodyTiming::BeforeResponse),
        ScriptedHttpExchange::response(
            http::Response::builder()
                .status(503)
                .header("x-response-revision", "latest-29")
                .body(
                    Full::new(Bytes::from_static(b"latest-rejection"))
                        .map_err(|never| match never {})
                        .boxed_unsync(),
                )
                .unwrap(),
            RequestBodyTiming::BeforeResponse,
            Box::new(async { Ok(()) }),
        ),
    ]);
    let mut host = retry_host(status_predicate(), 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { Ok(()) },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("exhaustion lost the response")
    };
    assert_eq!(result.response.status(), 503);
    assert_eq!(
        result.response.headers()["x-response-revision"],
        "latest-29"
    );
    assert_eq!(
        result
            .response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
        b"latest-rejection".as_slice()
    );
    assert_eq!(host.recorded, 1);
    assert_eq!(transport.take_requests().len(), 2);
    assert_eq!(transport.invalidation_count(), 1);
}

#[test]
#[timeout("10s")]
async fn retry_driver_returns_early_response_with_pending_transmission() {
    let (release, ready) = oneshot::channel();
    let body = StreamBody::new(futures::stream::once(async {
        ready.await.unwrap();
        Ok(http_body::Frame::data(Bytes::from_static(
            b"late charge upload",
        )))
    }))
    .boxed_unsync();
    let physical = physical_request(&HashMap::new(), body, true);
    let transport = InMemoryHttpTransport::new([response(201, RequestBodyTiming::WithIo)]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { Ok(()) },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("early headers were not returned")
    };
    assert_eq!(result.response.status(), 201);
    let mut io = Box::into_pin(result.io);
    assert!(io.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    io.await.unwrap();
    let captures = transport.take_requests();
    let CapturedRequestBody::Complete(body) = captures[0].request.body().as_ref() else {
        panic!("the returned I/O future did not own transmission");
    };
    assert_eq!(body.bytes, b"late charge upload".as_slice());
    assert_eq!(host.recorded, 0);
    assert_eq!(transport.invalidation_count(), 0);
}

#[test]
#[timeout("10s")]
async fn retry_driver_refuses_status_retry_when_body_cannot_be_replayed() {
    let physical = physical_request(&HashMap::new(), request_body(), false);
    let transport = InMemoryHttpTransport::new([
        response(503, RequestBodyTiming::BeforeResponse),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(status_predicate(), 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { panic!("a nonreplayable body must not reach backoff") },
    )
    .await;
    let DriverExit::HttpError(failure) = exit else {
        panic!("a nonreplayable body was retried")
    };
    assert!(matches!(failure.error, ErrorCode::InternalError(Some(_))));
    assert_eq!(transport.take_requests().len(), 1);
}

#[test]
#[timeout("10s")]
async fn retry_driver_preserves_interrupt_during_send() {
    let body = StreamBody::new(futures::stream::pending::<
        Result<http_body::Frame<Bytes>, ErrorCode>,
    >())
    .boxed_unsync();
    let physical = physical_request(&HashMap::new(), body, true);
    let transport = InMemoryHttpTransport::new([response(201, RequestBodyTiming::BeforeResponse)]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(async { InterruptKind::Restart }),
        |_| async { panic!("an interrupted send must not wait for retry") },
    )
    .await;
    assert!(matches!(
        exit,
        DriverExit::Interrupted(InterruptKind::Restart)
    ));
    assert_eq!(host.recorded, 0);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 1);
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Reading
    ));
}

#[test]
#[timeout("10s")]
async fn retry_driver_preserves_interrupt_during_backoff() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error(wasmtime_wasi_http::Error::ConnectionRefused),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { Err(InterruptKind::ShardLost) },
    )
    .await;
    assert!(matches!(
        exit,
        DriverExit::Interrupted(InterruptKind::ShardLost)
    ));
    assert_eq!(host.recorded, 1);
    assert_eq!(transport.take_requests().len(), 1);
}

#[test]
#[timeout("10s")]
async fn retry_driver_does_not_dispatch_when_retry_recording_is_refused() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error(wasmtime_wasi_http::Error::ConnectionRefused),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(Predicate::True, 1);
    host.append_error = Some(OplogError::Payload("retry append refused".to_string()));
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { panic!("a refused retry append must not reach backoff") },
    )
    .await;
    let DriverExit::HttpError(failure) = exit else {
        panic!("the refused retry lost its send error")
    };
    assert!(matches!(failure.error, ErrorCode::ConnectionRefused));
    assert!(failure.semantic_override.is_none());
    assert_eq!(host.recorded, 0);
    assert_eq!(transport.take_requests().len(), 1);
}

#[test]
#[timeout("10s")]
async fn physical_attempt_maps_send_errors() {
    let physical = physical_request(
        &HashMap::new(),
        Empty::new().map_err(|never| match never {}).boxed_unsync(),
        true,
    );
    let transport = InMemoryHttpTransport::new([ScriptedHttpExchange::send_error(
        wasmtime_wasi_http::Error::ConnectionRefused,
    )]);
    assert!(matches!(
        physical.attempt(&transport).await,
        Err(ErrorCode::ConnectionRefused)
    ));
}

#[test]
#[timeout("10s")]
async fn physical_attempt_maps_body_and_io_errors_independently() {
    let physical = physical_request(
        &HashMap::new(),
        Empty::new().map_err(|never| match never {}).boxed_unsync(),
        true,
    );
    let body = StreamBody::new(futures::stream::once(async {
        Err::<http_body::Frame<Bytes>, _>(wasmtime_wasi_http::Error::HttpProtocolError)
    }))
    .boxed_unsync();
    let transport = InMemoryHttpTransport::new([ScriptedHttpExchange::response(
        http::Response::new(body),
        RequestBodyTiming::BeforeResponse,
        Box::new(async { Err(wasmtime_wasi_http::Error::ConnectionReadTimeout) }),
    )]);
    let (response, io, _) = physical.attempt(&transport).await.unwrap();
    assert!(matches!(
        response.into_body().collect().await,
        Err(ErrorCode::HttpProtocolError)
    ));
    assert!(matches!(
        Box::into_pin(io).await,
        Err(ErrorCode::ConnectionReadTimeout)
    ));
}

struct RetryHost {
    policies: Box<[NamedRetryPolicy]>,
    state: Option<RetryPolicyState>,
    recorded: u32,
    append_error: Option<OplogError>,
}

fn retry_host(predicate: Predicate, max_retries: u32) -> RetryHost {
    RetryHost {
        policies: Box::new([NamedRetryPolicy {
            name: "charge-retry".to_string(),
            priority: 17,
            predicate,
            policy: RetryPolicy::CountBox {
                max_retries,
                inner: Box::new(RetryPolicy::Immediate),
            },
        }]),
        state: None,
        recorded: 0,
        append_error: None,
    }
}

#[async_trait]
impl InFunctionRetryHost for RetryHost {
    fn current_retry_point(&self) -> OplogIndex {
        OplogIndex::from_u64(709)
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        self.policies.to_vec()
    }

    async fn current_retry_state_for(&self, _: OplogIndex) -> Option<RetryPolicyState> {
        self.state.clone()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            snapshotting_mode: false,
            max_in_function_retry_delay: Duration::from_secs(1),
            durable_wait: true,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        state: Option<RetryPolicyState>,
        _: Timestamp,
    ) -> Result<(), OplogError> {
        assert_eq!(retry_from, OplogIndex::from_u64(709));
        if let Some(error) = &self.append_error {
            return Err(error.clone());
        }
        self.recorded += 1;
        self.state = state;
        Ok(())
    }
}

fn request_body() -> UnsyncBoxBody<Bytes, ErrorCode> {
    Full::new(Bytes::from_static(b"charge=93&currency=EUR"))
        .map_err(|never| match never {})
        .boxed_unsync()
}

#[test]
#[timeout("10s")]
async fn retry_driver_reuses_derived_key_after_unpolled_send_failure() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error(wasmtime_wasi_http::Error::ConnectionRefused),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { Ok(()) },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("the production retry driver did not return the successful response");
    };
    assert_eq!(result.response.status(), 201);
    assert_eq!(
        result
            .response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
        b"charge-accepted".as_slice()
    );
    Box::into_pin(result.io).await.unwrap();
    assert_eq!(host.recorded, 1);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 2);
    let independently_derived = Uuid::new_v5(
        &Uuid::parse_str("7b536e18-3ac7-4cf1-9931-b552f4ef728d").unwrap(),
        b"oplog-index-317",
    );
    assert_eq!(
        independently_derived.to_string(),
        "65a19192-a492-51cb-bd6f-2b403992359a"
    );
    captures.iter().for_each(|capture| {
        assert_eq!(
            capture.request.headers()["idempotency-key"],
            "65a19192-a492-51cb-bd6f-2b403992359a"
        );
        assert_eq!(
            capture
                .request
                .headers()
                .get_all("idempotency-key")
                .iter()
                .count(),
            1
        );
    });
    assert_eq!(
        captures[0].request.headers()["idempotency-key"],
        captures[1].request.headers()["idempotency-key"]
    );
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Unpolled
    ));
    let CapturedRequestBody::Complete(body) = captures[1].request.body().as_ref() else {
        panic!("the successful resend did not consume the body");
    };
    assert_eq!(body.bytes, b"charge=93&currency=EUR".as_slice());
}

#[test]
#[timeout("10s")]
async fn retry_driver_reuses_derived_key_after_consumed_send_failure() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error_after_request_body(
            wasmtime_wasi_http::Error::HttpProtocolError,
        ),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { Ok(()) },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("the consumed upload was not retried");
    };
    assert_eq!(result.response.status(), 201);
    assert_eq!(
        result
            .response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
        b"charge-accepted".as_slice()
    );
    Box::into_pin(result.io).await.unwrap();
    assert_eq!(host.recorded, 1);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 2);
    captures.iter().for_each(|capture| {
        assert_eq!(
            capture.request.headers()["idempotency-key"],
            "65a19192-a492-51cb-bd6f-2b403992359a"
        );
        assert_eq!(
            capture
                .request
                .headers()
                .get_all("idempotency-key")
                .iter()
                .count(),
            1
        );
        let CapturedRequestBody::Complete(body) = capture.request.body().as_ref() else {
            panic!("the send failure must follow healthy upload intake");
        };
        assert_eq!(body.bytes, b"charge=93&currency=EUR".as_slice());
    });
}

#[test]
#[timeout("10s")]
async fn retry_driver_drains_early_rejection_before_backoff() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        response(503, RequestBodyTiming::WithIo),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(status_predicate(), 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async {
            assert_eq!(transport.invalidation_count(), 1);
            assert!(physical.body.frames_consumed());
            assert!(physical.body.can_replay_after_send_failure());
            Ok(())
        },
    )
    .await;
    let DriverExit::Response(result) = exit else {
        panic!("the drained body was not resent")
    };
    assert_eq!(result.response.status(), 201);
    assert_eq!(transport.invalidation_count(), 1);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 2);
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Unpolled
    ));
    let CapturedRequestBody::Complete(body) = captures[1].request.body().as_ref() else {
        panic!("the retry did not transmit the drained body");
    };
    assert_eq!(body.bytes, b"charge=93&currency=EUR".as_slice());
}

#[test]
#[timeout("10s")]
async fn retry_driver_preserves_body_recording_recovery_error() {
    let tasks = Arc::new(crate::worker::tasks::RuntimeAppendTasks::default());
    tasks.seal_and_wait().await;
    let mut physical = physical_request(&HashMap::new(), request_body(), true);
    physical.body = DurableRequestBody::new(
        request_body(),
        FrameTestOplog::new(),
        OplogIndex::from_u64(941),
        true,
        tasks,
    );
    let transport = InMemoryHttpTransport::new([response(201, RequestBodyTiming::BeforeResponse)]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { panic!("a body recording failure must not reach backoff") },
    )
    .await;
    let DriverExit::RecoveryError(error) = exit else {
        panic!("body recording failure became an HTTP result")
    };
    assert!(matches!(
        error.downcast_ref::<WorkerExecutorError>(),
        Some(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert_eq!(host.recorded, 0);
    assert_eq!(transport.take_requests().len(), 1);
}

#[test]
#[timeout("10s")]
async fn retry_driver_retains_status_fallback_response_and_semantic_override() {
    let physical = physical_request(&HashMap::new(), request_body(), false);
    let transport = InMemoryHttpTransport::new([response(503, RequestBodyTiming::BeforeResponse)]);
    let mut host = retry_host(status_predicate(), 1);
    host.policies[0].policy = RetryPolicy::TimeBox {
        limit: Duration::from_secs(60),
        inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(30))),
    };
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        true,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { panic!("a status fallback must not wait in-function") },
    )
    .await;
    let DriverExit::StatusFallback(fallback) = exit else {
        panic!("the status fallback was lost")
    };
    assert_eq!(fallback.response.response.status(), 503);
    let payload = fallback
        .semantic_override
        .expect("TimeBox fallback must retain its evaluated state");
    assert_eq!(payload.retry_from, OplogIndex::from_u64(709));
    assert_eq!(payload.policy_name, "charge-retry");
    assert!(
        matches!(payload.verdict, crate::durable_host::durability::SemanticTrapRetryVerdict::Retry(delay) if delay == Duration::from_secs(30))
    );
    assert_eq!(host.recorded, 0);
    assert_eq!(transport.take_requests().len(), 1);
    assert_eq!(transport.invalidation_count(), 0);
}

#[test]
#[timeout("10s")]
async fn retry_driver_does_not_retry_when_inline_retry_is_ineligible() {
    let physical = physical_request(&HashMap::new(), request_body(), true);
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error(wasmtime_wasi_http::Error::ConnectionRefused),
        response(201, RequestBodyTiming::BeforeResponse),
    ]);
    let mut host = retry_host(Predicate::True, 1);
    let exit = drive_send(
        &physical,
        &transport,
        &mut host,
        false,
        ("POST", "http://example.com/charge/93"),
        || Box::pin(futures::future::pending::<InterruptKind>()),
        |_| async { panic!("an ineligible send must not reach backoff") },
    )
    .await;
    let DriverExit::HttpError(failure) = exit else {
        panic!("the ineligible send lost its error")
    };
    assert!(matches!(failure.error, ErrorCode::ConnectionRefused));
    assert_eq!(host.recorded, 0);
    assert_eq!(transport.take_requests().len(), 1);
}
