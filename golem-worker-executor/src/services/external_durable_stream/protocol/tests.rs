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

use super::super::{DefaultExternalDurableStreamService, ExternalDurableStreamService};
use super::*;
use axum::body::{Body, Bytes};
use axum::http::{Method, Response as HttpResponse, Uri};
use axum::routing::any;
use futures::StreamExt;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use test_r::test;

#[test]
fn codec_reservations_preserve_bounds_and_reject_overflow() {
    for max in [1, 17, 65_537, 8 * 1024 * 1024] {
        for transport in [
            DurableStreamTransport::CatchUp,
            DurableStreamTransport::LongPoll,
        ] {
            assert_eq!(
                read_memory_reservation(transport, max),
                Some(6 * max as u64 + 2_097_152)
            );
        }
        assert_eq!(
            append_memory_reservation(max),
            Some(6 * max as u64 + 2_097_152)
        );
        assert_eq!(
            read_memory_reservation(DurableStreamTransport::Sse, max),
            Some(32 * max as u64 + 2_097_152)
        );
    }
    if usize::BITS == 64 {
        assert_eq!(append_memory_reservation(usize::MAX), None);
        assert_eq!(
            read_memory_reservation(DurableStreamTransport::Sse, usize::MAX),
            None
        );
        let last = ((u64::MAX - 2_097_152) / 32) as usize;
        assert!(read_memory_reservation(DurableStreamTransport::Sse, last).is_some());
        assert_eq!(
            read_memory_reservation(DurableStreamTransport::Sse, last + 1),
            None
        );
    }
}

#[test]
fn sse_retained_capacity_fits_reservation_at_growth_boundaries() {
    for max in [1, 17, 65_537] {
        for base64 in [false, true] {
            let mut parser = SseParser::new(max, base64, DurableStreamMode::Bytes);
            let value = "x".repeat(max);
            let encoded = if base64 {
                base64::engine::general_purpose::STANDARD.encode(value)
            } else {
                value
            };
            // Retain the completed payload while growing a subsequent event's line/data.
            // Unknown events and comments still consume the bounded framing budget.
            let wire = format!(
                "event: data\ndata: {encoded}\n\nevent: unknown\ndata: {}\n:{}",
                "c".repeat(max),
                "d".repeat(max)
            );
            let reservation = read_memory_reservation(DurableStreamTransport::Sse, max).unwrap();
            for byte in wire.bytes() {
                assert!(parser.push(byte).unwrap().is_none());
                let retained = parser.line.capacity()
                    + parser.data.capacity()
                    + parser.event.capacity()
                    + parser.pending.as_ref().map_or(0, Vec::capacity);
                // Add one replacement allocation while either growable buffer reallocates.
                let transient = parser.line.capacity().max(parser.data.capacity()) * 2;
                assert!((retained + transient) as u64 <= reservation);
            }
            assert_eq!(parser.pending.as_ref().unwrap().len(), max);
        }
    }
}

#[test]
async fn injected_service_uses_single_attempts_and_does_not_follow_redirects() {
    let target = Server::new(vec![]).await;
    let server = Server::new(vec![
        response(307, &[("location", &target.url)], Body::empty()),
        response(503, &[], Body::from("private-server-error")),
        response(
            204,
            &[("producer-epoch", "7"), ("producer-seq", "11")],
            Body::empty(),
        ),
    ])
    .await;
    let service: Arc<dyn ExternalDurableStreamService> =
        Arc::new(DefaultExternalDurableStreamService::new().unwrap());
    assert_eq!(
        service
            .read_batch(&read_request(&server.url), Some("private-token"), 1024)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::ProtocolError
    );
    let error = service
        .append_batch(&append_request(&server.url), None, 1024)
        .await
        .unwrap_err();
    assert_eq!(error.kind, DurableStreamErrorKind::Unavailable);
    assert!(!error.message.contains("private"));
    let receipt = service
        .append_batch(&append_request(&server.url), None, 1024)
        .await
        .unwrap();
    assert_eq!(
        (receipt.epoch, receipt.sequence, receipt.next_offset),
        (7, 11, None)
    );
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    assert!(target.requests.lock().unwrap().is_empty());
}

struct Captured {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new(responses: Vec<HttpResponse<Body>>) -> Self {
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = axum::Router::new().route(
            "/stream",
            any(move |request: axum::extract::Request| {
                let responses = responses.clone();
                let captured = captured.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                    captured.lock().unwrap().push(Captured {
                        method: parts.method,
                        uri: parts.uri,
                        headers: parts.headers,
                        body,
                    });
                    responses
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| response(500, &[], Body::empty()))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

fn client() -> Client {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .build()
        .unwrap()
}

fn response(status: u16, headers: &[(&str, &str)], body: Body) -> HttpResponse<Body> {
    let mut builder = HttpResponse::builder().status(status);
    for (key, value) in headers {
        builder = builder.header(*key, *value);
    }
    builder.body(body).unwrap()
}

fn read_request(url: &str) -> DurableStreamReadRequest {
    DurableStreamReadRequest {
        url: url.to_owned(),
        checkpoint: DurableStreamCheckpoint {
            offset: "-1".to_owned(),
            cursor: None,
        },
        mode: DurableStreamMode::Json,
        transport: DurableStreamTransport::CatchUp,
        content_type: None,
        timeout_ms: 2000,
    }
}

fn append_request(url: &str) -> DurableStreamAppendRequest {
    DurableStreamAppendRequest {
        url: url.to_owned(),
        content_type: "application/json".to_owned(),
        payload: DurableStreamAppendPayload::Json(vec![
            "[1,2]".to_owned(),
            "9007199254740993".to_owned(),
        ]),
        producer: DurableStreamProducer {
            id: "producer-A".to_owned(),
            epoch: 7,
            sequence: 11,
        },
        close: false,
        timeout_ms: 2000,
    }
}

#[test]
fn validates_url_headers_and_transport_before_io() {
    for url in [
        "http://example.com/stream",
        "https://user:secret@host/stream",
        "https://@host/stream",
        "https:///@host/stream",
        "https://host\\@other/stream",
        "https://host/stream#fragment",
        "file:///stream",
        "https://host/stream?%6fffset=bad",
        "https://host/stream?live=sse",
        "https://host/stream?cursor=1",
        "https://host/stream\n",
    ] {
        assert_eq!(
            validate_read(&read_request(url), 1024).unwrap_err().kind,
            DurableStreamErrorKind::InvalidRequest,
            "{url}"
        );
    }
    for url in [
        "http://localhost/stream",
        "http://127.99.2.1/stream",
        "http://[::1]/stream",
        "https://example.com/stream?tenant=a",
    ] {
        assert!(validate_read(&read_request(url), 1024).is_ok(), "{url}");
    }
    let mut request = read_request("https://example.com/stream");
    request.checkpoint.offset = "opaque+token".to_owned();
    request.checkpoint.cursor = Some("a&b=c".to_owned());
    request.transport = DurableStreamTransport::LongPoll;
    assert_eq!(
        validate_read(&request, 1024).unwrap().query(),
        Some("cursor=a%26b%3Dc&live=long-poll&offset=opaque%2Btoken")
    );
    for offset in ["", "a/b", "a=b", "a&b", "a?b", "a,b", "a b", "a\rb"] {
        request.checkpoint.offset = offset.to_owned();
        assert!(validate_read(&request, 1024).is_err());
    }
    request.checkpoint.offset = "now".to_owned();
    assert!(validate_read(&request, 1024).is_err());
    request.transport = DurableStreamTransport::Sse;
    request.checkpoint.offset = "tail".to_owned();
    assert!(validate_read(&request, 1024).is_err());
    request.content_type = Some("application/json".to_owned());
    assert!(validate_read(&request, 1024).is_ok());
    request.timeout_ms = 300_001;
    assert!(validate_read(&request, 1024).is_err());
}

#[test]
fn append_validates_exact_json_and_preserves_lexemes() {
    let mut request = append_request("https://example.com/stream");
    let expected = b"[[1,2],9007199254740993]";
    assert!(validate_append(&request, expected.len()).is_ok());
    assert_eq!(append_body(&request.payload).unwrap(), expected);
    assert_eq!(
        validate_append(&request, expected.len() - 1)
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
    for value in ["1 2", "{", "NaN", "", "[1,]", "true false"] {
        request.payload = DurableStreamAppendPayload::Json(vec![value.to_owned()]);
        assert_eq!(
            validate_append(&request, 1024).unwrap_err().kind,
            DurableStreamErrorKind::InvalidRequest
        );
    }
    request.payload = DurableStreamAppendPayload::Json(vec!["1e99999".to_owned(), "[]".to_owned()]);
    assert!(validate_append(&request, 1024).is_ok());
    assert_eq!(append_body(&request.payload).unwrap(), b"[1e99999,[]]");
    request.payload = DurableStreamAppendPayload::Bytes(b"[1]".to_vec());
    assert!(validate_append(&request, 1024).is_err());
    request.content_type = "application/octet-stream".to_owned();
    request.producer.epoch = MAX_INTEGER;
    request.producer.sequence = MAX_INTEGER;
    assert!(validate_append(&request, 1024).is_ok());
    request.producer.sequence += 1;
    assert!(validate_append(&request, 1024).is_err());
    request.producer.sequence = 0;
    request.producer.epoch += 1;
    assert!(validate_append(&request, 1024).is_err());
    request.producer.epoch = 0;
    request.producer.id = "p\r\nAuthorization: hidden".to_owned();
    assert!(validate_append(&request, 1024).is_err());
}

#[test]
async fn rejects_invalid_bodies_and_credentials_without_a_request() {
    let server = Server::new(vec![]).await;
    let client = client();
    let service = DefaultExternalDurableStreamService::new().unwrap();
    let mut request = append_request(&server.url);
    for value in ["1 2".to_owned(), "[".repeat(1022)] {
        request.payload = DurableStreamAppendPayload::Json(vec![value]);
        // Preflight cannot allocate the JSON parser's payload-sized nesting stack.
        assert!(preflight_append(&request, 1024).is_ok());
        assert_eq!(
            service
                .append_batch(&request, None, 1024)
                .await
                .unwrap_err()
                .kind,
            DurableStreamErrorKind::InvalidRequest
        );
        assert!(server.requests.lock().unwrap().is_empty());
    }
    request.payload = DurableStreamAppendPayload::Json(vec![]);
    assert!(preflight_append(&request, 1024).is_err());
    assert!(append_batch(&client, &request, None, 1024).await.is_err());
    request = append_request(&server.url);
    assert_eq!(
        preflight_append(&request, 2).unwrap_err().kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
    assert_eq!(
        append_batch(&client, &request, None, 2)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
    let error = read_batch(
        &client,
        &read_request(&server.url),
        Some("hidden\r\nx"),
        1024,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, DurableStreamErrorKind::InvalidRequest);
    assert!(!error.message.contains("hidden"));
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
async fn reads_final_payload_now_and_empty_open_long_poll() {
    let server = Server::new(vec![
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "tail-9"),
                ("stream-closed", "true"),
            ],
            Body::from("[[1,2],9007199254740993]"),
        ),
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "tail-10"),
                ("stream-up-to-date", "true"),
            ],
            Body::from("[]"),
        ),
        response(
            204,
            &[
                ("stream-next-offset", "tail-10"),
                ("stream-cursor", "cursor-11"),
                ("stream-up-to-date", "true"),
            ],
            Body::empty(),
        ),
    ])
    .await;
    let client = client();
    let mut request = read_request(&server.url);
    let batch = read_batch(&client, &request, Some("secret-token"), 1024)
        .await
        .unwrap();
    assert_eq!(batch.payload, b"[[1,2],9007199254740993]");
    assert!(batch.closed && batch.up_to_date);
    assert_eq!(batch.next.offset, "tail-9");
    request.checkpoint.offset = "now".to_owned();
    let batch = read_batch(&client, &request, None, 1024).await.unwrap();
    assert_eq!(batch.payload, b"[]");
    assert!(!batch.closed);
    request.checkpoint = batch.next;
    request.content_type = Some(batch.content_type);
    request.transport = DurableStreamTransport::LongPoll;
    let batch = read_batch(&client, &request, None, 1024).await.unwrap();
    assert_eq!(batch.payload, b"[]");
    assert!(batch.up_to_date && !batch.closed);
    assert_eq!(batch.next.cursor.as_deref(), Some("cursor-11"));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(requests[0].headers["authorization"], "Bearer secret-token");
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("range"))
    );
    assert_eq!(
        requests[2].uri.query(),
        Some("live=long-poll&offset=tail-10")
    );
}

#[test]
async fn rejects_nonarray_missing_checkpoint_and_nonempty_now() {
    let server = Server::new(vec![
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "tail"),
            ],
            Body::from("{\"a\":1}"),
        ),
        response(
            200,
            &[("content-type", "application/json")],
            Body::from("[]"),
        ),
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "now"),
            ],
            Body::from("[]"),
        ),
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "tail"),
                ("stream-up-to-date", "true"),
            ],
            Body::from("[1]"),
        ),
    ])
    .await;
    let mut request = read_request(&server.url);
    let client = client();
    for _ in 0..3 {
        assert_eq!(
            read_batch(&client, &request, None, 1024)
                .await
                .unwrap_err()
                .kind,
            DurableStreamErrorKind::ProtocolError
        );
    }
    request.checkpoint.offset = "now".to_owned();
    assert_eq!(
        read_batch(&client, &request, None, 1024)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::ProtocolError
    );
}

#[test]
async fn append_headers_acknowledgements_and_close_only() {
    let server = Server::new(vec![
        response(
            200,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "11"),
                ("stream-next-offset", "tail-a"),
            ],
            Body::empty(),
        ),
        response(
            204,
            &[("producer-epoch", "7"), ("producer-seq", "11")],
            Body::empty(),
        ),
        response(
            204,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "12"),
                ("stream-next-offset", "tail-a"),
                ("stream-closed", "true"),
            ],
            Body::empty(),
        ),
    ])
    .await;
    let client = client();
    let mut request = append_request(&server.url);
    let receipt = append_batch(&client, &request, None, 1024).await.unwrap();
    assert_eq!((receipt.epoch, receipt.sequence), (7, 11));
    assert_eq!(receipt.next_offset.as_deref(), Some("tail-a"));
    let duplicate = append_batch(&client, &request, None, 1024).await.unwrap();
    assert_eq!((duplicate.epoch, duplicate.sequence), (7, 11));
    assert_eq!(duplicate.next_offset, None);
    request.payload = DurableStreamAppendPayload::Json(vec![]);
    request.close = true;
    request.producer.sequence = 12;
    assert!(
        append_batch(&client, &request, None, 1024)
            .await
            .unwrap()
            .closed
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, Method::POST);
    assert_eq!(requests[0].body, "[[1,2],9007199254740993]");
    assert_eq!(requests[1].body, requests[0].body);
    for request in requests.iter() {
        assert_eq!(request.headers["producer-id"], "producer-A");
        assert_eq!(request.headers["producer-epoch"], "7");
    }
    assert_eq!(requests[2].headers["producer-seq"], "12");
    assert_eq!(requests[2].headers["stream-closed"], "true");
    assert!(requests[2].body.is_empty());
}

#[test]
async fn json_close_only_accepts_empty_bytes_but_rejects_unframed_data() {
    let server = Server::new(vec![response(
        204,
        &[
            ("producer-epoch", "7"),
            ("producer-seq", "11"),
            ("stream-closed", "true"),
        ],
        Body::empty(),
    )])
    .await;
    let mut request = append_request(&server.url);
    request.content_type = "Application/JSON; charset=utf-8".into();
    request.payload = DurableStreamAppendPayload::Bytes(vec![]);
    assert!(validate_append(&request, 1024).is_err());
    request.close = true;
    request.payload = DurableStreamAppendPayload::Bytes(b"[]".to_vec());
    assert!(validate_append(&request, 1024).is_err());
    request.payload = DurableStreamAppendPayload::Bytes(vec![]);
    let receipt = append_batch(&client(), &request, None, 1024).await.unwrap();
    assert!(receipt.closed);
    assert_eq!(receipt.next_offset, None);
    assert_eq!((receipt.epoch, receipt.sequence), (7, 11));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.is_empty());
    assert_eq!(requests[0].headers["stream-closed"], "true");
    assert_eq!(requests[0].headers["content-type"], request.content_type);
}

#[test]
async fn ack_must_match_nonpipelined_tuple() {
    let server = Server::new(vec![
        response(
            200,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "12"),
                ("stream-next-offset", "tail"),
            ],
            Body::empty(),
        ),
        response(
            200,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "10"),
                ("stream-next-offset", "tail"),
            ],
            Body::empty(),
        ),
        response(
            200,
            &[
                ("producer-epoch", "8"),
                ("producer-seq", "11"),
                ("stream-next-offset", "tail"),
            ],
            Body::empty(),
        ),
        response(
            200,
            &[("producer-epoch", "7"), ("stream-next-offset", "tail")],
            Body::empty(),
        ),
    ])
    .await;
    let client = client();
    let request = append_request(&server.url);
    assert_eq!(
        append_batch(&client, &request, None, 1024)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::ProducerDiverged
    );
    for _ in 0..3 {
        assert_eq!(
            append_batch(&client, &request, None, 1024)
                .await
                .unwrap_err()
                .kind,
            DurableStreamErrorKind::ProtocolError
        );
    }
}

#[test]
async fn errors_are_typed_sanitized_and_not_retried() {
    let cases = [
        (
            403,
            vec![("producer-epoch", "9")],
            DurableStreamErrorKind::Fenced,
        ),
        (403, vec![], DurableStreamErrorKind::PermissionDenied),
        (
            409,
            vec![("producer-expected-seq", "8")],
            DurableStreamErrorKind::SequenceConflict,
        ),
        (
            409,
            vec![("stream-closed", "true")],
            DurableStreamErrorKind::Closed,
        ),
        (409, vec![], DurableStreamErrorKind::ProtocolError),
        (410, vec![], DurableStreamErrorKind::Gone),
        (404, vec![], DurableStreamErrorKind::NotFound),
        (413, vec![], DurableStreamErrorKind::PayloadTooLarge),
        (
            429,
            vec![("retry-after", "3")],
            DurableStreamErrorKind::RateLimited,
        ),
        (503, vec![], DurableStreamErrorKind::Unavailable),
    ];
    let server = Server::new(
        cases
            .iter()
            .map(|(status, headers, _)| {
                response(
                    *status,
                    headers,
                    Body::from("server-secret-token private body"),
                )
            })
            .collect(),
    )
    .await;
    let client = client();
    let request = append_request(&format!("{}?private=url-token", server.url));
    for (status, _, kind) in &cases {
        let error = append_batch(&client, &request, Some("bearer-token"), 1024)
            .await
            .unwrap_err();
        assert_eq!(&error.kind, kind);
        assert!(!format!("{error:?}").contains("token"));
        if *status == 429 {
            assert_eq!(error.retry_after_ms, Some(3000));
        }
        assert_eq!(error.is_retryable(), *status == 429 || *status == 503);
    }
    assert_eq!(server.requests.lock().unwrap().len(), cases.len());
}

#[test]
fn retry_after_dates_are_recordable_relative_delays() {
    let now = "2026-09-17T12:00:00.250Z".parse::<DateTime<Utc>>().unwrap();
    assert_eq!(
        retry_after("Thu, 17 Sep 2026 12:00:03 GMT", now),
        Some(2750)
    );
    assert_eq!(retry_after("Thu, 17 Sep 2026 11:59:59 GMT", now), Some(0));
    assert_eq!(retry_after("4", now), Some(4000));
    assert_eq!(retry_after("18446744073709551615", now), None);
    assert_eq!(retry_after("hidden", now), None);
}

#[test]
async fn bounded_chunked_bytes_and_truncated_body_do_not_advance() {
    let chunks = || {
        Body::from_stream(futures::stream::iter(vec![
            Ok::<_, Infallible>(Bytes::from_static(&[0, 255])),
            Ok(Bytes::from_static(&[19, 4, 8])),
        ]))
    };
    let broken = Body::from_stream(
        futures::stream::iter(vec![
            Ok(Bytes::from_static(&[0, 255])),
            Err(std::io::Error::other("private-transport-error")),
        ])
        .then(|value| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            value
        }),
    );
    let headers = [
        ("content-type", "application/octet-stream"),
        ("stream-next-offset", "tail"),
    ];
    let server = Server::new(vec![
        response(200, &headers, chunks()),
        response(200, &headers, chunks()),
        response(200, &headers, broken),
    ])
    .await;
    let mut request = read_request(&server.url);
    request.mode = DurableStreamMode::Bytes;
    let client = client();
    assert_eq!(
        read_batch(&client, &request, None, 5)
            .await
            .unwrap()
            .payload,
        [0, 255, 19, 4, 8]
    );
    assert_eq!(
        read_batch(&client, &request, None, 4)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
    let error = read_batch(&client, &request, None, 1024).await.unwrap_err();
    assert_eq!(error.kind, DurableStreamErrorKind::Transport);
    assert!(!error.message.contains("private"));
    assert_eq!(request.checkpoint.offset, "-1");
}

fn parse_sse(
    bytes: &[u8],
    max: usize,
    base64: bool,
    mode: DurableStreamMode,
) -> Result<Option<(Vec<u8>, Control)>, DurableStreamError> {
    let mut parser = SseParser::new(max, base64, mode);
    for byte in bytes {
        if let Some(pair) = parser.push(*byte)? {
            return Ok(Some(pair));
        }
    }
    Ok(None)
}

#[test]
fn sse_fragmented_utf8_multiline_crlf_bom_comments_and_final_control() {
    let wire = "\u{feff}: keepalive\r\nevent: data\r\ndata: [\r\ndata: \"árvíz\",9007199254740993]\r\n\r\n: ping\r\nevent: control\r\ndata: {\"streamNextOffset\":\"tail-11\",\r\ndata: \"streamClosed\":true}\r\n\r\nevent: data\ndata: malformed ignored remainder\n\n";
    let (payload, control) = parse_sse(wire.as_bytes(), 1024, false, DurableStreamMode::Json)
        .unwrap()
        .unwrap();
    assert_eq!(payload, "[\n\"árvíz\",9007199254740993]".as_bytes());
    assert_eq!(control.stream_next_offset, "tail-11");
    assert!(control.stream_closed);
    assert!(control.stream_cursor.is_none());
    let bare_cr =
        b"event: control\rdata: {\"streamNextOffset\":\"tail\",\"streamCursor\":\"7\"}\r\r";
    assert!(
        parse_sse(bare_cr, 2, false, DurableStreamMode::Json)
            .unwrap()
            .is_some()
    );
}

#[test]
fn sse_requires_complete_control_and_decoded_limit_allows_base64_padding() {
    let wire = b"event: data\ndata: AP8T\ndata: BAg=\n\nevent: control\ndata: {\"streamNextOffset\":\"tail\",\"streamCursor\":\"c7\"}\n\n";
    let (payload, _) = parse_sse(wire, 5, true, DurableStreamMode::Bytes)
        .unwrap()
        .unwrap();
    assert_eq!(payload, [0, 255, 19, 4, 8]);
    assert_eq!(
        parse_sse(wire, 4, true, DurableStreamMode::Bytes)
            .err()
            .unwrap()
            .kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
    for suffix in [
        "",
        "event: control\ndata: {\"streamNextOffset\":\"tail\"}",
        "event: control\ndata: {\"streamNextOffset\":\"tail\"}\n",
    ] {
        let wire = format!("event: data\ndata: []\n\n{suffix}");
        assert!(
            parse_sse(wire.as_bytes(), 1024, false, DurableStreamMode::Json)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        parse_sse(
            b"event: data\ndata: []\n\nevent: data\ndata: [1]\n\n",
            1024,
            false,
            DurableStreamMode::Json
        )
        .is_err()
    );
    assert!(
        parse_sse(
            b"event: data\ndata: %%%%\n\n",
            1024,
            true,
            DurableStreamMode::Bytes
        )
        .is_err()
    );
    let mut parser = SseParser::new(1, false, DurableStreamMode::Bytes);
    for _ in 0..parser.wire_limit / 2 {
        parser.push(b':').unwrap();
        parser.push(b'\n').unwrap();
    }
    assert_eq!(
        parser.push(b':').err().unwrap().kind,
        DurableStreamErrorKind::PayloadTooLarge
    );
}

#[test]
async fn sse_http_stops_after_first_pair_without_waiting_for_eof() {
    let wire = b"event: data\ndata: AP8TBAg=\n\nevent: control\ndata: {\"streamNextOffset\":\"tail\",\"streamCursor\":\"c2\",\"upToDate\":true}\n\n";
    let body = Body::from_stream(
        futures::stream::iter(
            wire.iter()
                .map(|byte| Ok::<_, Infallible>(Bytes::copy_from_slice(&[*byte])))
                .collect::<Vec<_>>(),
        )
        .chain(futures::stream::pending()),
    );
    let headers = [
        ("content-type", "text/event-stream; charset=utf-8"),
        ("stream-sse-data-encoding", "base64"),
    ];
    let server = Server::new(vec![
        response(200, &headers, body),
        response(200, &headers, Body::from("event: data\ndata: AP8TBAg=\n\n")),
    ])
    .await;
    let mut request = read_request(&server.url);
    request.mode = DurableStreamMode::Bytes;
    request.content_type = Some("application/octet-stream".to_owned());
    request.transport = DurableStreamTransport::Sse;
    let client = client();
    let batch = read_batch(&client, &request, None, 5).await.unwrap();
    assert_eq!(batch.payload, [0, 255, 19, 4, 8]);
    assert_eq!(batch.content_type, "application/octet-stream");
    assert_eq!(batch.next.offset, "tail");
    assert_eq!(batch.next.cursor.as_deref(), Some("c2"));
    assert!(batch.up_to_date && !batch.closed);
    assert_eq!(
        read_batch(&client, &request, None, 5)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::Transport
    );
}

#[test]
async fn deadlines_cover_body_reads_and_redirects_do_not_forward_bearer() {
    let target = Server::new(vec![]).await;
    let body = || Body::from_stream(futures::stream::pending::<Result<Bytes, Infallible>>());
    let server = Server::new(vec![
        response(307, &[("location", &target.url)], Body::empty()),
        response(
            200,
            &[
                ("content-type", "application/json"),
                ("stream-next-offset", "tail"),
            ],
            body(),
        ),
        response(
            200,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "11"),
                ("stream-next-offset", "tail"),
            ],
            body(),
        ),
    ])
    .await;
    let client = client();
    let mut read = read_request(&server.url);
    let error = read_batch(&client, &read, Some("confidential"), 1024)
        .await
        .unwrap_err();
    assert_eq!(error.kind, DurableStreamErrorKind::ProtocolError);
    assert!(!error.message.contains("confidential"));
    assert!(target.requests.lock().unwrap().is_empty());
    read.timeout_ms = 100;
    assert_eq!(
        read_batch(&client, &read, None, 1024)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::Timeout
    );
    let mut append = append_request(&server.url);
    append.timeout_ms = 100;
    assert_eq!(
        append_batch(&client, &append, None, 1024)
            .await
            .unwrap_err()
            .kind,
        DurableStreamErrorKind::Timeout
    );
    assert_eq!(server.requests.lock().unwrap().len(), 3);
}

#[test]
async fn sse_control_only_closed_and_invalid_checkpoints() {
    let bodies = [
        "event: control\ndata: {\"streamNextOffset\":\"tail\",\"streamClosed\":true}\n\n",
        "event: control\ndata: {\"streamNextOffset\":\"tail\"}\n\n",
        "event: control\ndata: {\"streamNextOffset\":\"now\",\"streamClosed\":true}\n\n",
        "event: control\ndata: {\"streamNextOffset\":\"tail\",\"streamCursor\":\"c\\r\\nx\"}\n\n",
        "event: control\ndata: {\"streamNextOffset\":\"tail\",\"streamClosed\":\"true\"}\n\n",
    ];
    let server = Server::new(
        bodies
            .iter()
            .map(|body| {
                response(
                    200,
                    &[("content-type", "text/event-stream")],
                    Body::from(*body),
                )
            })
            .collect(),
    )
    .await;
    let client = client();
    let mut request = read_request(&server.url);
    request.transport = DurableStreamTransport::Sse;
    request.content_type = Some("application/json".to_owned());
    let batch = read_batch(&client, &request, None, 2).await.unwrap();
    assert_eq!(batch.payload, b"[]");
    assert!(batch.closed && batch.up_to_date);
    assert_eq!(batch.next.offset, "tail");
    for _ in 1..bodies.len() {
        assert_eq!(
            read_batch(&client, &request, None, 2)
                .await
                .unwrap_err()
                .kind,
            DurableStreamErrorKind::ProtocolError
        );
    }
}

#[test]
fn serializable_dtos_roundtrip_without_numeric_or_checkpoint_loss() {
    use golem_common::schema::{FromSchema, IntoSchema};

    fn roundtrip<T>(value: T)
    where
        T: desert_rust::BinaryCodec + IntoSchema + FromSchema + std::fmt::Debug + PartialEq,
    {
        let bytes = desert_rust::serialize_to_byte_vec(&value).unwrap();
        assert_eq!(desert_rust::deserialize::<T>(&bytes).unwrap(), value);
        assert_eq!(T::from_value(&value.to_value()).unwrap(), value);
    }

    roundtrip(append_request("https://example.com/stream"));
    let mut request = read_request("https://example.com/stream");
    request.checkpoint.cursor = Some("cursor".to_owned());
    request.transport = DurableStreamTransport::Sse;
    request.content_type = Some("application/json".to_owned());
    roundtrip(request);
    roundtrip(DurableStreamBatch {
        payload: vec![0, 255, 19],
        content_type: "application/octet-stream".to_owned(),
        next: DurableStreamCheckpoint {
            offset: "opaque-11".to_owned(),
            cursor: Some("cursor-13".to_owned()),
        },
        up_to_date: false,
        closed: true,
    });
    roundtrip(DurableStreamAppendReceipt {
        next_offset: Some("tail".to_owned()),
        epoch: MAX_INTEGER,
        sequence: 8,
        closed: false,
    });
    roundtrip(DurableStreamAppendReceipt {
        next_offset: None,
        epoch: 7,
        sequence: 11,
        closed: false,
    });
    roundtrip(DurableStreamError {
        kind: DurableStreamErrorKind::SequenceConflict,
        message: "HTTP 409".to_owned(),
        retry_after_ms: Some(2711),
        producer_epoch: Some(7),
        expected_sequence: Some(11),
    });
}

#[test]
async fn bytes_append_and_close_is_unframed_and_read_media_type_is_pinned() {
    let server = Server::new(vec![
        response(
            200,
            &[
                ("producer-epoch", "7"),
                ("producer-seq", "11"),
                ("stream-next-offset", "tail"),
                ("stream-closed", "true"),
            ],
            Body::empty(),
        ),
        response(
            200,
            &[
                ("content-type", "image/png"),
                ("stream-next-offset", "tail"),
            ],
            Body::from(vec![0, 255, 19, 4, 8]),
        ),
    ])
    .await;
    let client = client();
    let mut append = append_request(&server.url);
    append.content_type = "application/octet-stream".to_owned();
    append.payload = DurableStreamAppendPayload::Bytes(vec![0, 255, 19, 4, 8]);
    append.close = true;
    assert!(
        append_batch(&client, &append, None, 5)
            .await
            .unwrap()
            .closed
    );
    let mut read = read_request(&server.url);
    read.mode = DurableStreamMode::Bytes;
    read.content_type = Some("application/octet-stream".to_owned());
    assert_eq!(
        read_batch(&client, &read, None, 5).await.unwrap_err().kind,
        DurableStreamErrorKind::ProtocolError
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0].body.as_ref(), &[0, 255, 19, 4, 8]);
    assert_eq!(requests[0].headers["stream-closed"], "true");
    assert_eq!(
        requests[0].headers["content-type"],
        "application/octet-stream"
    );
}
