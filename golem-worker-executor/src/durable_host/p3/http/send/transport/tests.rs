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
use crate::durable_host::durability::DurableExecutionState;
use crate::durable_host::p3::http::test_support::{FrameTestOplog, TestHttpCtx};
use crate::durable_host::{DurableHttpHooks, IdempotencyKeyPosition};
use crate::services::oplog::OplogError;
use async_trait::async_trait;
use futures::{FutureExt, StreamExt};
use golem_common::model::{Predicate, RetryPolicy, RetryPolicyState, Timestamp};
use http::HeaderValue;
use http_body_util::Empty;
use std::time::Duration;
use test_r::{test, timeout};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use wasmtime_wasi_http::WasiHttpView;

fn physical_request(
    authority: String,
    guest_key: Option<&'static str>,
) -> PhysicalSendRequest<crate::workerctx::default::Context> {
    let mut store = wasmtime::Store::new(&wasmtime::Engine::default(), TestHttpCtx::default());
    let mut headers = HeaderMap::new();
    if let Some(key) = guest_key {
        headers.insert("idempotency-key", HeaderValue::from_static(key));
    }
    let fields =
        wasmtime_wasi_http::FieldMap::new_immutable(store.data_mut().http().hooks, headers);
    let (request, transmission) = wasmtime_wasi_http::p3::Request::new(
        http::Method::POST,
        Some(http::uri::Scheme::HTTP),
        Some(authority.parse().unwrap()),
        Some(http::uri::PathAndQuery::from_static("/charge/93")),
        fields,
        None,
        Empty::new().map_err(|never| match never {}).boxed_unsync(),
    );
    let req = store.data_mut().table.push(request).unwrap();
    let position = IdempotencyKeyPosition {
        parent: Uuid::parse_str("177db03d-3234-4a04-8d03-e8d042348abd")
            .unwrap()
            .into(),
        logical: Some(OplogIndex::from_u64(17)),
    };
    let managed = golem_outgoing_http_headers(
        store.data().table.get(&req).unwrap(),
        vec![("traceparent".to_string(), "trace-charge-93".to_string())],
        true,
        OplogIndex::from_u64(41),
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
            OplogIndex::from_u64(41),
            true,
            Default::default(),
        ),
        options: options.map(|options| *options),
        final_transmission_tx: oneshot::channel().0,
        _phantom: PhantomData,
    }
}

async fn receive_requests(listener: TcpListener, responses: &[bool]) -> Box<[String]> {
    futures::stream::iter(responses)
        .then(|respond| {
            let listener = &listener;
            async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let headers = futures::stream::unfold(BufReader::new(&mut socket), |mut reader| async move {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
                    (line != "\r\n").then_some((line, reader))
                })
                .collect::<String>()
                .await;
                if *respond {
                    socket.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await.unwrap();
                }
                headers
            }
        })
        .collect::<Vec<_>>()
        .await
        .into_boxed_slice()
}

fn assert_wire_key(request: &str, expected: &str) {
    assert_eq!(request.lines().next(), Some("POST /charge/93 HTTP/1.1"));
    let keys = request
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("idempotency-key")
                .then_some(value.trim())
        })
        .collect::<Vec<_>>();
    assert_eq!(keys, [expected]);
    assert!(
        request
            .lines()
            .any(|line| line == "traceparent: trace-charge-93")
    );
}

#[test]
#[timeout("10s")]
async fn physical_attempt_preserves_guest_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let physical = physical_request(
        listener.local_addr().unwrap().to_string(),
        Some("business-charge-93-revision-7"),
    );
    let peer = tokio::spawn(async move { receive_requests(listener, &[true]).await });
    let (response, io, _) = physical.attempt().await.unwrap();
    assert_eq!(response.status(), 201);
    response.into_body().collect().await.unwrap();
    Box::into_pin(io).await.unwrap();
    let requests = peer.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_wire_key(&requests[0], "business-charge-93-revision-7");
}

struct RetryRecorder {
    state: Option<RetryPolicyState>,
    recorded: u32,
}

#[async_trait]
impl InFunctionRetryHost for RetryRecorder {
    fn current_retry_point(&self) -> OplogIndex {
        OplogIndex::from_u64(41)
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        vec![NamedRetryPolicy {
            name: "transport-retry".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 1,
                inner: Box::new(RetryPolicy::Immediate),
            },
        }]
    }

    async fn current_retry_state_for(&self, _: OplogIndex) -> Option<RetryPolicyState> {
        self.state.clone()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            snapshotting_mode: false,
            max_in_function_retry_delay: Duration::from_secs(1),
            durable_wait: false,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        _: OplogIndex,
        state: Option<RetryPolicyState>,
        _: Timestamp,
    ) -> Result<(), OplogError> {
        self.state = state;
        self.recorded += 1;
        Ok(())
    }
}

#[test]
#[timeout("10s")]
async fn physical_attempt_retry_reuses_one_derived_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let physical = physical_request(listener.local_addr().unwrap().to_string(), None);
    let peer = tokio::spawn(async move { receive_requests(listener, &[false, true]).await });
    let Err(error) = physical.attempt().await else {
        panic!("the peer must close the first request without a response")
    };
    assert_eq!(
        classify_serializable_http_error_code(&serialize_error_code(&error)),
        HostFailureKind::Transient
    );
    let mut retry = InFunctionRetryState::new();
    let mut host = RetryRecorder {
        state: None,
        recorded: 0,
    };
    assert!(matches!(
        retry
            .decide_retry_with_properties(
                &mut host,
                "http::client::send",
                &RetryContext::http_with_response(
                    "POST",
                    &physical.uri.to_string(),
                    None,
                    "transient"
                ),
            )
            .await,
        AsyncRetryDecision::RetryAfterDelay(Duration::ZERO)
    ));
    assert_eq!(host.recorded, 1);
    let (response, io, _) = physical.attempt().await.unwrap();
    assert_eq!(response.status(), 201);
    response.into_body().collect().await.unwrap();
    Box::into_pin(io).await.unwrap();
    let requests = peer.await.unwrap();
    assert_eq!(requests.len(), 2);
    // UUIDv5 of the parent UUID and the literal UTF-8 name oplog-index-17.
    requests
        .iter()
        .for_each(|request| assert_wire_key(request, "8169f767-c476-5171-b4a3-f295e3ec9373"));
}
