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
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use http_body::Frame;
use http_body_util::{Full, StreamBody};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

fn full(bytes: &'static [u8]) -> WasiBody {
    Full::new(Bytes::from_static(bytes))
        .map_err(Error::from)
        .boxed_unsync()
}

fn unpollable_body() -> WasiBody {
    StreamBody::new(stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Frame<Bytes>, Error>>> {
            panic!("a pre-send failure must not poll the body")
        },
    ))
    .boxed_unsync()
}

#[test]
#[timeout("10s")]
async fn memory_response_does_not_wait_for_upload_and_captures_are_snapshots() {
    let transport = InMemoryHttpTransport::new([ScriptedHttpExchange::response(
        http::Response::new(full(b"accepted")),
        RequestBodyTiming::WithIo,
        Box::new(async { Ok(()) }),
    )]);
    let (release, ready) = oneshot::channel();
    let upload = StreamBody::new(stream::once(async move {
        ready.await.unwrap();
        Ok(Frame::data(Bytes::from_static(b"late upload")))
    }))
    .boxed_unsync();
    let (response, io, invalidate) = transport
        .send(http::Request::new(upload), None)
        .await
        .unwrap();
    assert_eq!(transport.invalidation_count(), 0);
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 1);
    invalidate();
    assert_eq!(transport.invalidation_count(), 1);
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Unpolled
    ));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "accepted"
    );
    let mut io = Box::into_pin(io);
    assert!(io.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    io.await.unwrap();
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Unpolled
    ));
    assert!(transport.take_requests().is_empty());
}

#[test]
#[timeout("10s")]
async fn memory_send_error_waits_for_eof_and_preserves_body_errors() {
    let transport = InMemoryHttpTransport::new([
        ScriptedHttpExchange::send_error_after_request_body(Error::HttpProtocolError),
        ScriptedHttpExchange::send_error_after_request_body(Error::HttpProtocolError),
    ]);
    let (release, eof) = oneshot::channel();
    let upload = StreamBody::new(
        stream::iter([Ok(Frame::data(Bytes::from_static(b"healthy-prefix")))]).chain(
            stream::once(async move {
                eof.await.unwrap();
                None::<Result<Frame<Bytes>, Error>>
            })
            .filter_map(futures::future::ready),
        ),
    )
    .boxed_unsync();
    let mut send = Box::pin(transport.send(http::Request::new(upload), None));
    assert!(send.as_mut().now_or_never().is_none());
    let snapshots = transport.take_requests();
    assert_eq!(snapshots.len(), 1);
    assert!(matches!(
        snapshots[0].request.body().as_ref(),
        CapturedRequestBody::Reading
    ));
    release.send(()).unwrap();
    assert!(matches!(send.await, Err(Error::HttpProtocolError)));
    assert!(matches!(
        snapshots[0].request.body().as_ref(),
        CapturedRequestBody::Reading
    ));
    assert!(transport.take_requests().is_empty());

    let failed_upload = StreamBody::new(stream::iter([
        Ok(Frame::data(Bytes::from_static(b"failed-prefix"))),
        Err(Error::HttpRequestBodySize(Some(73))),
    ]))
    .boxed_unsync();
    assert!(matches!(
        transport
            .send(http::Request::new(failed_upload), None)
            .await,
        Err(Error::HttpRequestBodySize(Some(73)))
    ));
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 1);
    assert!(matches!(
        captures[0].request.body().as_ref(),
        CapturedRequestBody::Failed
    ));
}

#[test]
#[timeout("10s")]
async fn memory_scripted_send_error_and_exhaustion_do_not_read_the_body() {
    let transport = InMemoryHttpTransport::new([ScriptedHttpExchange::send_error(
        Error::ConnectionReadTimeout,
    )]);
    assert!(matches!(
        transport
            .send(http::Request::new(unpollable_body()), options())
            .await,
        Err(Error::ConnectionReadTimeout)
    ));
    assert!(matches!(
        transport
            .send(http::Request::new(unpollable_body()), None)
            .await,
        Err(Error::InternalError(Some(message))) if message == "HTTP transport script exhausted"
    ));
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0].options, options());
    assert_eq!(captures[1].options, None);
    captures.iter().for_each(|capture| {
        assert!(matches!(
            capture.request.body().as_ref(),
            CapturedRequestBody::Unpolled
        ));
    });
    assert!(transport.take_requests().is_empty());
}

#[test]
#[timeout("10s")]
async fn memory_scripted_io_error_does_not_replace_the_response() {
    let transport = InMemoryHttpTransport::new([ScriptedHttpExchange::response(
        reply(202, full(b"accepted")),
        RequestBodyTiming::WithIo,
        Box::new(async { Err(Error::ConnectionWriteTimeout) }),
    )]);
    let uri = http::Uri::from_static("http://example.com/charges?logical=61");
    let (response, io, _invalidate) = transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
        .unwrap();
    assert_eq!(transport.invalidation_count(), 0);
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "accepted"
    );
    assert!(matches!(
        Box::into_pin(io).await,
        Err(Error::ConnectionWriteTimeout)
    ));
    let captures = transport.take_requests();
    assert_eq!(captures.len(), 1);
    assert_request(&captures[0], &uri);
}

#[derive(Clone, Copy)]
enum Adapter {
    Memory,
    Pooled,
    Unpooled,
}

enum Observations {
    Memory(Arc<InMemoryHttpTransport>),
    Peer(JoinHandle<Box<[CapturedRequest]>>),
}

struct Fixture {
    transport: Arc<dyn HttpTransport>,
    uri: http::Uri,
    observations: Observations,
}

async fn fixture<F, Fut>(
    adapter: Adapter,
    script: impl IntoIterator<Item = ScriptedHttpExchange>,
    serve: F,
) -> Fixture
where
    F: FnOnce(TcpListener) -> Fut + Send + 'static,
    Fut: Future<Output = Box<[CapturedRequest]>> + Send + 'static,
{
    match adapter {
        Adapter::Memory => {
            let transport = Arc::new(InMemoryHttpTransport::new(script));
            Fixture {
                transport: transport.clone(),
                uri: http::Uri::from_static("http://127.0.0.1:7319/charges?logical=61"),
                observations: Observations::Memory(transport),
            }
        }
        Adapter::Pooled | Adapter::Unpooled => {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let uri = format!(
                "http://{}/charges?logical=61",
                listener.local_addr().unwrap()
            )
            .parse()
            .unwrap();
            let pool = matches!(adapter, Adapter::Pooled)
                .then(|| HttpConnectionPool::new(Default::default()));
            Fixture {
                transport: Arc::new(WasiHttpTransport::new(pool)),
                uri,
                observations: Observations::Peer(tokio::spawn(serve(listener))),
            }
        }
    }
}

async fn observations(fixture: Fixture) -> Box<[CapturedRequest]> {
    match fixture.observations {
        Observations::Memory(transport) => transport.take_requests(),
        Observations::Peer(peer) => peer.await.unwrap(),
    }
}

fn options() -> Option<RequestOptions> {
    Some(RequestOptions {
        connect_timeout: Some(Duration::from_secs(7)),
        first_byte_timeout: Some(Duration::from_secs(9)),
        between_bytes_timeout: Some(Duration::from_secs(11)),
    })
}

fn request(uri: &http::Uri, body: WasiBody) -> http::Request<WasiBody> {
    http::Request::builder()
        .method("POST")
        .uri(uri.clone())
        .version(http::Version::HTTP_11)
        .header("host", uri.authority().unwrap().as_str())
        .header("idempotency-key", "business-charge-619")
        .header("traceparent", "00-7319-619-01")
        .header("content-length", "15")
        .body(body)
        .unwrap()
}

fn reply(status: u16, body: WasiBody) -> http::Response<WasiBody> {
    http::Response::builder()
        .status(status)
        .header("x-receipt", "receipt-937")
        .body(body)
        .unwrap()
}

fn scripted_reply(status: u16) -> ScriptedHttpExchange {
    ScriptedHttpExchange::response(
        reply(status, full(b"receipt-body-937")),
        RequestBodyTiming::BeforeResponse,
        Box::new(async { Ok(()) }),
    )
}

// The peer accepts only the fixed HTTP/1.1 request shape used below.
async fn read_head(socket: TcpStream) -> (BufReader<TcpStream>, http::Request<()>) {
    let mut reader = BufReader::new(socket);
    let lines = stream::try_unfold(&mut reader, |reader| async move {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        assert!(line.len() < 8192);
        Ok::<_, std::io::Error>((line != "\r\n").then_some((line.into_boxed_str(), reader)))
    })
    .try_collect::<Vec<_>>()
    .await
    .unwrap()
    .into_boxed_slice();
    let mut lines = lines.iter();
    let mut first = lines.next().unwrap().split_whitespace();
    let method: http::Method = first.next().unwrap().parse().unwrap();
    let path = first.next().unwrap();
    assert_eq!(first.next().unwrap(), "HTTP/1.1");
    let headers: http::HeaderMap = lines
        .map(|line| {
            let (name, value) = line.trim_end().split_once(':').unwrap();
            (
                name.parse::<http::HeaderName>().unwrap(),
                value.trim_start().parse::<http::HeaderValue>().unwrap(),
            )
        })
        .collect();
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "http://{}{path}",
            headers["host"].to_str().unwrap()
        ))
        .version(http::Version::HTTP_11)
        .body(())
        .unwrap();
    *request.headers_mut() = headers;
    (reader, request)
}

async fn read_request_from_peer(listener: &TcpListener) -> (BufReader<TcpStream>, CapturedRequest) {
    let (socket, _) = listener.accept().await.unwrap();
    let (mut reader, head) = read_head(socket).await;
    let length = head.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    (
        reader,
        CapturedRequest {
            request: head.map(|()| {
                Arc::new(CapturedRequestBody::Complete(Arc::new(CapturedBody {
                    bytes: body.into(),
                    trailers: None,
                })))
            }),
            options: None,
        },
    )
}

fn assert_request(capture: &CapturedRequest, uri: &http::Uri) {
    assert_eq!(capture.request.method(), http::Method::POST);
    assert_eq!(capture.request.uri(), uri);
    assert_eq!(capture.request.version(), http::Version::HTTP_11);
    assert_eq!(
        capture.request.headers()["idempotency-key"],
        "business-charge-619"
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
    assert_eq!(capture.request.headers()["traceparent"], "00-7319-619-01");
    let CapturedRequestBody::Complete(body) = capture.request.body().as_ref() else {
        panic!("request body did not complete");
    };
    assert_eq!(body.bytes, "charge-body-731");
    assert!(body.trailers.is_none());
}

async fn preserves_head_body_options_and_response(adapter: Adapter) {
    let fixture = fixture(adapter, [scripted_reply(201)], |listener| async move {
        let (mut reader, request) = read_request_from_peer(&listener).await;
        reader
            .get_mut()
            .write_all(b"HTTP/1.1 201 Created\r\nConnection: close\r\nX-Receipt: receipt-937\r\nContent-Length: 16\r\n\r\nreceipt-body-937")
            .await
            .unwrap();
        Box::new([request]) as Box<[_]>
    })
    .await;
    let uri = fixture.uri.clone();
    let (response, io, _invalidate) = fixture
        .transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["x-receipt"], "receipt-937");
    let io = tokio::spawn(Box::into_pin(io));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "receipt-body-937"
    );
    io.await.unwrap().unwrap();
    let captures = observations(fixture).await;
    assert_eq!(captures.len(), 1);
    assert_request(&captures[0], &uri);
    if matches!(adapter, Adapter::Memory) {
        assert_eq!(captures[0].options, options());
    }
}

async fn rejected_status_then_success_preserves_requests(adapter: Adapter) {
    let fixture = fixture(
        adapter,
        [
            ScriptedHttpExchange::response(
                reply(503, full(b"")),
                RequestBodyTiming::BeforeResponse,
                Box::new(async { Ok(()) }),
            ),
            scripted_reply(201),
        ],
        |listener| async move {
            let (mut first, first_request) = read_request_from_peer(&listener).await;
            first
                .get_mut()
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nX-Receipt: receipt-937\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            // An empty body leaves this connection reusable unless the sender invalidates it.
            assert_eq!(first.read(&mut [0; 1]).await.unwrap(), 0);
            let (mut second, second_request) = read_request_from_peer(&listener).await;
            second
                .get_mut()
                .write_all(b"HTTP/1.1 201 Created\r\nConnection: close\r\nX-Receipt: receipt-937\r\nContent-Length: 16\r\n\r\nreceipt-body-937")
                .await
                .unwrap();
            Box::new([first_request, second_request]) as Box<[_]>
        },
    )
    .await;
    let uri = fixture.uri.clone();
    let (response, io, invalidate) = fixture
        .transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    invalidate();
    if let Observations::Memory(transport) = &fixture.observations {
        assert_eq!(transport.invalidation_count(), 1);
    }
    drop(response);
    drop(io);
    drop(invalidate);
    let (response, io, _invalidate) = fixture
        .transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let io = tokio::spawn(Box::into_pin(io));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "receipt-body-937"
    );
    io.await.unwrap().unwrap();
    if let Observations::Memory(transport) = &fixture.observations {
        assert_eq!(transport.invalidation_count(), 1);
    }
    let captures = observations(fixture).await;
    assert_eq!(captures.len(), 2);
    captures
        .iter()
        .for_each(|capture| assert_request(capture, &uri));
}

fn p3_error(error: &Error) -> ErrorCode {
    wasmtime_wasi_http::p3::error_to_p3(wasmtime_wasi_http::default_hooks(), error)
}

async fn consumed_upload_send_error_then_success(adapter: Adapter) {
    let fixture = fixture(
        adapter,
        [
            ScriptedHttpExchange::send_error_after_request_body(Error::HttpProtocolError),
            scripted_reply(201),
        ],
        |listener| async move {
            let (first, first_request) = read_request_from_peer(&listener).await;
            drop(first);
            let (mut second, second_request) = read_request_from_peer(&listener).await;
            second
                .get_mut()
                .write_all(b"HTTP/1.1 201 Created\r\nConnection: close\r\nX-Receipt: receipt-937\r\nContent-Length: 16\r\n\r\nreceipt-body-937")
                .await
                .unwrap();
            Box::new([first_request, second_request]) as Box<[_]>
        },
    )
    .await;
    let uri = fixture.uri.clone();
    let Err(error) = fixture
        .transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
    else {
        panic!("the peer closed before sending response headers");
    };
    assert!(matches!(p3_error(&error), ErrorCode::HttpProtocolError));
    let (response, io, _invalidate) = fixture
        .transport
        .send(request(&uri, full(b"charge-body-731")), options())
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let io = tokio::spawn(Box::into_pin(io));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "receipt-body-937"
    );
    io.await.unwrap().unwrap();
    let captures = observations(fixture).await;
    assert_eq!(captures.len(), 2);
    captures.iter().for_each(|capture| {
        assert_request(capture, &uri);
        if matches!(adapter, Adapter::Memory) {
            assert_eq!(capture.options, options());
        }
    });
}

async fn pre_send_error_does_not_poll_body(adapter: Adapter) {
    let transport: Arc<dyn HttpTransport> = match adapter {
        Adapter::Memory => Arc::new(InMemoryHttpTransport::new([
            ScriptedHttpExchange::send_error(Error::HttpRequestUriInvalid),
        ])),
        Adapter::Pooled => Arc::new(WasiHttpTransport::new(Some(HttpConnectionPool::new(
            Default::default(),
        )))),
        Adapter::Unpooled => Arc::new(WasiHttpTransport::new(None)),
    };
    let result = transport
        .send(
            http::Request::builder()
                .uri("/missing-authority")
                .body(unpollable_body())
                .unwrap(),
            options(),
        )
        .await;
    let Err(error) = result else {
        panic!("a relative URI must fail before send");
    };
    assert!(matches!(error, Error::HttpRequestUriInvalid));
}

async fn response_body_error_keeps_the_response(adapter: Adapter) {
    let (release, hold) = oneshot::channel::<()>();
    let failed_body = StreamBody::new(stream::once(async {
        Err::<Frame<Bytes>, _>(Error::ConnectionReadTimeout)
    }))
    .boxed_unsync();
    let fixture = fixture(
        adapter,
        [ScriptedHttpExchange::response(
            reply(202, failed_body),
            RequestBodyTiming::BeforeResponse,
            Box::new(async { Ok(()) }),
        )],
        |listener| async move {
            let (mut reader, request) = read_request_from_peer(&listener).await;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 202 Accepted\r\nX-Receipt: receipt-937\r\nContent-Length: 16\r\n\r\n")
                .await
                .unwrap();
            hold.await.unwrap();
            Box::new([request]) as Box<[_]>
        },
    )
    .await;
    let uri = fixture.uri.clone();
    let (response, io, _invalidate) = fixture
        .transport
        .send(
            request(&uri, full(b"charge-body-731")),
            Some(RequestOptions {
                between_bytes_timeout: Some(Duration::from_millis(1)),
                ..options().unwrap()
            }),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(response.headers()["x-receipt"], "receipt-937");
    let io = tokio::spawn(Box::into_pin(io));
    let mut body = response.into_body();
    assert!(matches!(
        body.frame().await,
        Some(Err(Error::ConnectionReadTimeout))
    ));
    drop(body);
    let _ = release.send(());
    // The default I/O future also drives the response connection; the pooled future does not.
    let _ = io.await.unwrap();
    let captures = observations(fixture).await;
    assert_eq!(captures.len(), 1);
    assert_request(&captures[0], &uri);
}

async fn early_response_keeps_late_transmission_error(adapter: Adapter) {
    let fixture = fixture(
        adapter,
        [ScriptedHttpExchange::response(
            reply(202, full(b"early")),
            RequestBodyTiming::WithIo,
            Box::new(async { Ok(()) }),
        )],
        |listener| async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (mut reader, _) = read_head(socket).await;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 202 Accepted\r\nX-Receipt: receipt-937\r\nContent-Length: 16\r\n\r\nearly")
                .await
                .unwrap();
            assert_eq!(reader.read(&mut [0; 1]).await.unwrap(), 0);
            Box::new([]) as Box<[_]>
        },
    )
    .await;
    let (release, hold) = oneshot::channel();
    let upload = StreamBody::new(stream::once(async move {
        hold.await.unwrap();
        Err::<Frame<Bytes>, _>(Error::HttpRequestBodySize(Some(73)))
    }))
    .boxed_unsync();
    let (response, io, _invalidate) = fixture
        .transport
        .send(request(&fixture.uri, upload), options())
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let mut io = Box::into_pin(io);
    assert!(io.as_mut().now_or_never().is_none());
    let io = tokio::spawn(io);
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "early"
    );
    release.send(()).unwrap();
    let error = io.await.unwrap().unwrap_err();
    assert!(matches!(
        p3_error(&error),
        ErrorCode::HttpRequestBodySize(Some(73))
    ));
    drop(body);
    let captures = observations(fixture).await;
    if matches!(adapter, Adapter::Memory) {
        assert_eq!(captures.len(), 1);
        assert!(matches!(
            captures[0].request.body().as_ref(),
            CapturedRequestBody::Failed
        ));
    }
}

async fn contract_suite(adapter: Adapter) {
    preserves_head_body_options_and_response(adapter).await;
    rejected_status_then_success_preserves_requests(adapter).await;
    consumed_upload_send_error_then_success(adapter).await;
    pre_send_error_does_not_poll_body(adapter).await;
    response_body_error_keeps_the_response(adapter).await;
    early_response_keeps_late_transmission_error(adapter).await;
}

#[test]
#[timeout("30s")]
async fn contract_memory() {
    contract_suite(Adapter::Memory).await;
}

#[test]
#[timeout("30s")]
async fn contract_wasi_pooled() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    contract_suite(Adapter::Pooled).await;
}

#[test]
#[timeout("30s")]
async fn contract_wasi_unpooled() {
    contract_suite(Adapter::Unpooled).await;
}
