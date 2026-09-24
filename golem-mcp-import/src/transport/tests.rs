use super::*;
use futures::stream::{self, BoxStream};
use golem_schema::schema::SchemaValue;
use http_body::Frame;
use http_body_util::{StreamBody, combinators::UnsyncBoxBody};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
};
use test_r::test;

type TestBody = UnsyncBoxBody<Bytes, TransportError>;

#[derive(Default)]
struct Upstream {
    requests: Vec<Request<Bytes>>,
    responses: VecDeque<Response<TestBody>>,
    rejection: Option<TransportError>,
}

impl HttpSend for Upstream {
    type Body = TestBody;
    type Error = TransportError;

    async fn send(
        &mut self,
        request: Request<Bytes>,
    ) -> Result<Response<TestBody>, TransportError> {
        self.requests.push(request);
        if let Some(rejection) = self.rejection.take() {
            return Err(rejection);
        }
        Ok(self
            .responses
            .pop_front()
            .expect("unexpected extra HTTP request"))
    }
}

fn body(chunks: impl IntoIterator<Item = Bytes>) -> TestBody {
    let frames: Vec<_> = chunks.into_iter().map(|b| Ok(Frame::data(b))).collect();
    StreamBody::new(stream::iter(frames)).boxed_unsync()
}

fn response(result: Value) -> Response<TestBody> {
    numbered_response(1, result)
}

fn numbered_response(id: i64, result: Value) -> Response<TestBody> {
    raw_json(
        json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
        StatusCode::OK,
    )
}

fn raw_json(json: String, status: StatusCode) -> Response<TestBody> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body([Bytes::from(json)]))
        .unwrap()
}

fn client(limits: Limits) -> Client {
    Client::new("https://tools.example/mcp", None, limits).unwrap()
}

fn tool() -> ProjectedTool {
    ProjectedTool::new(
        &json!({
            "name":"Lookup_name", "inputSchema":{"type":"object", "properties":{
                "tenant":{"type":"string", "x-mcp-header":"Tenant"}
            },"required":["tenant"],"additionalProperties":false}
        }),
        "lookup-name",
        Default::default(),
    )
    .unwrap()
}

fn input() -> SchemaValue {
    SchemaValue::Record {
        fields: vec![SchemaValue::String("公司".into())],
    }
}

#[test]
async fn direct_call_has_snapshot_headers_and_preserves_the_complete_result() {
    let result = json!({"resultType":"complete", "content":[{"type":"text","text":"ok"}],
        "structuredContent":{"id":7}, "_meta":{"remote":true}, "extension":[3,1,4]});
    let mut upstream = Upstream {
        responses: VecDeque::from([response(result.clone())]),
        ..Default::default()
    };
    let client = client(Limits::default())
        .with_bearer("dummy-token")
        .unwrap();
    assert_eq!(
        client
            .call_tool(&mut upstream, &tool(), &input(), Some("durable-key"))
            .await
            .unwrap(),
        result
    );
    assert_eq!(upstream.requests.len(), 1);
    let request = &upstream.requests[0];
    assert_eq!(request.method(), http::Method::POST);
    assert_eq!(request.headers()[header::ACCEPT_ENCODING], "identity");
    assert_eq!(request.headers()["Mcp-Method"], "tools/call");
    assert_eq!(request.headers()["Mcp-Name"], "Lookup_name");
    assert_eq!(request.headers()["Mcp-Param-Tenant"], "=?base64?5YWs5Y+4?=");
    assert_eq!(request.headers()["Idempotency-Key"], "durable-key");
    assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
    assert!(!format!("{:?}", request.headers()).contains("dummy-token"));
    assert!(!request.headers().contains_key("Mcp-Session-Id"));
    assert!(!request.headers().contains_key("Last-Event-ID"));
    let payload: Value = serde_json::from_slice(request.body()).unwrap();
    assert_eq!(payload["params"]["arguments"], json!({"tenant":"公司"}));
    assert_eq!(
        payload["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        PROTOCOL_VERSION
    );
    assert_eq!(
        payload["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"],
        json!({})
    );
}

#[test]
async fn listing_preserves_invalid_definitions_and_fails_atomically_on_later_pages() {
    let mut upstream = Upstream {
        responses: VecDeque::from([
            response(json!({"tools":[{"name":"bad","inputSchema":42}],"nextCursor":"opaque?"})),
            numbered_response(2, json!({"tools":[{"name":"fine"}]})),
        ]),
        ..Default::default()
    };
    let listing = client(Limits::default())
        .list_tools(&mut upstream)
        .await
        .unwrap();
    assert_eq!(
        listing.tools,
        vec![
            json!({"name":"bad","inputSchema":42}),
            json!({"name":"fine"})
        ]
    );
    let second: Value = serde_json::from_slice(upstream.requests[1].body()).unwrap();
    assert_eq!(second["params"]["cursor"], "opaque?");
    assert_eq!(listing.protocol_version, PROTOCOL_VERSION);
    upstream.responses = VecDeque::from([
        response(json!({"tools":[{"name":"partial"}],"nextCursor":"next"})),
        raw_json("{}".into(), StatusCode::SERVICE_UNAVAILABLE),
    ]);
    assert_eq!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .unwrap_err(),
        TransportError::HttpStatus(503)
    );
    assert_eq!(upstream.requests.len(), 4);
}

#[test]
async fn pagination_bounds_count_whole_pages_and_detect_cycles() {
    for (limits, pages, expected) in [
        (
            Limits {
                pages: 1,
                ..Limits::default()
            },
            vec![json!({"tools":[],"nextCursor":"n"})],
            limit("pagination"),
        ),
        (
            Limits {
                tools: 1,
                ..Limits::default()
            },
            vec![json!({"tools":[{},{}]})],
            limit("tool count"),
        ),
        (
            Limits::default(),
            vec![
                json!({"tools":[],"nextCursor":"n"}),
                json!({"tools":[],"nextCursor":"n"}),
            ],
            protocol("repeated pagination cursor"),
        ),
    ] {
        let mut upstream = Upstream {
            responses: pages
                .into_iter()
                .enumerate()
                .map(|(index, page)| numbered_response(index as i64 + 1, page))
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            client(limits).list_tools(&mut upstream).await.unwrap_err(),
            expected
        );
    }
    let page = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}}).to_string();
    for (limit_bytes, success) in [(page.len(), true), (page.len() - 1, false)] {
        let mut upstream = Upstream {
            responses: VecDeque::from([raw_json(page.clone(), StatusCode::OK)]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits {
                listing_bytes: limit_bytes,
                ..Limits::default()
            })
            .list_tools(&mut upstream)
            .await
            .is_ok(),
            success
        );
    }
}

#[test]
async fn statuses_callbacks_and_ambiguous_network_failures_never_repeat_a_call() {
    for (status, payload) in [
        (StatusCode::UNAUTHORIZED, json!({})),
        (StatusCode::FOUND, json!({})),
        (
            StatusCode::NOT_FOUND,
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"removed"}}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32020,"message":"refresh headers"}}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32022,"message":"version","data":{"supported":["future"]}}}),
        ),
        (
            StatusCode::OK,
            json!({"jsonrpc":"2.0","id":1,"result":{"resultType":"input_required","inputRequests":{}}}),
        ),
        (
            StatusCode::OK,
            json!({"jsonrpc":"2.0","id":1,"method":"sampling/createMessage","params":{}}),
        ),
    ] {
        let mut upstream = Upstream {
            responses: VecDeque::from([raw_json(payload.to_string(), status)]),
            ..Default::default()
        };
        assert!(
            client(Limits::default())
                .call_tool(&mut upstream, &tool(), &input(), Some("same-key"))
                .await
                .is_err()
        );
        assert_eq!(upstream.requests.len(), 1);
    }
    for rejection in [
        TransportError::Network,
        TransportError::Denied,
        TransportError::QuotaExhausted,
    ] {
        let mut upstream = Upstream {
            rejection: Some(rejection),
            ..Default::default()
        };
        assert!(
            client(Limits::default())
                .call_tool(&mut upstream, &tool(), &input(), Some("same-key"))
                .await
                .is_err()
        );
        assert_eq!(upstream.requests.len(), 1);
    }
}

#[test]
async fn json_rpc_error_response_must_match_the_request_id() {
    let payload = json!({
        "jsonrpc":"2.0",
        "error":{"code":-32601,"message":"removed"}
    });
    let mut upstream = Upstream {
        responses: VecDeque::from([raw_json(payload.to_string(), StatusCode::OK)]),
        ..Default::default()
    };

    assert!(matches!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .unwrap_err(),
        TransportError::Protocol(_)
    ));
    for code in [-32700, -32600] {
        let message = json!({"jsonrpc":"2.0","error":{"code":code,"message":"malformed request"}});
        assert!(
            matches!(response_message(message.clone(), StatusCode::BAD_REQUEST, 77), Err(TransportError::Remote{code: actual, ..}) if actual == code)
        );
        let mut mismatched = message;
        mismatched["id"] = json!(78);
        assert!(matches!(
            response_message(mismatched, StatusCode::OK, 77),
            Err(TransportError::Protocol(_))
        ));
    }
}

#[test]
#[test_r::timeout("10s")]
async fn concurrent_clones_use_distinct_request_and_progress_ids() {
    struct Concurrent {
        barrier: Arc<tokio::sync::Barrier>,
        ids: Arc<std::sync::Mutex<Vec<i64>>>,
    }
    impl HttpSend for Concurrent {
        type Body = TestBody;
        type Error = TransportError;
        async fn send(
            &mut self,
            request: Request<Bytes>,
        ) -> Result<Response<TestBody>, TransportError> {
            let request: Value = serde_json::from_slice(request.body()).unwrap();
            let id = request["id"].as_i64().unwrap();
            assert_eq!(request["params"]["_meta"]["progressToken"], id);
            self.ids.lock().unwrap().push(id);
            self.barrier.wait().await;
            Ok(numbered_response(id, json!({"tools":[]})))
        }
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut left = Concurrent {
        barrier: barrier.clone(),
        ids: ids.clone(),
    };
    let mut right = Concurrent {
        barrier,
        ids: ids.clone(),
    };
    let client = client(Limits::default());
    let cloned = client.clone();
    let (a, b) = tokio::join!(client.list_tools(&mut left), cloned.list_tools(&mut right));
    a.unwrap();
    b.unwrap();
    let ids = ids.lock().unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
async fn json_limits_apply_to_wrappers_before_parsing_or_projection() {
    let raw =
        json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]},"ignored":"x".repeat(500)}).to_string();
    for (length, success) in [(raw.len(), true), (raw.len() - 1, false)] {
        let mut response = raw_json(raw.clone(), StatusCode::OK);
        *response.body_mut() = body(raw.as_bytes().chunks(13).map(Bytes::copy_from_slice));
        let mut upstream = Upstream {
            responses: VecDeque::from([response]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits {
                response_bytes: length,
                ..Limits::default()
            })
            .list_tools(&mut upstream)
            .await
            .is_ok(),
            success
        );
    }
    let mut response = response(json!({"tools":[]}));
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("99999999"));
    let mut upstream = Upstream {
        responses: VecDeque::from([response]),
        ..Default::default()
    };
    assert_eq!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .unwrap_err(),
        limit("response bytes")
    );
    assert!(parse_json(br#"{"quoted":"[\"[","x":[[]]}"#, 3).is_ok());
    assert_eq!(parse_json(b"[[[[]]]]", 3).unwrap_err(), limit("JSON depth"));
    assert!(parse_json(b"{} {}", 512).is_err());
    let deep = format!("{}0{}", "[".repeat(511), "]".repeat(511));
    assert!(parse_json(deep.as_bytes(), 512).is_ok());
}

#[test]
async fn accepts_case_insensitive_identity_content_encoding() {
    let mut encoded = response(json!({"tools":[]}));
    encoded.headers_mut().insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("Identity"),
    );
    let mut upstream = Upstream {
        responses: VecDeque::from([encoded]),
        ..Default::default()
    };
    assert!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .is_ok()
    );
}

#[test]
async fn accepts_optional_whitespace_around_identity_content_encoding() {
    let mut encoded = response(json!({"tools":[]}));
    encoded.headers_mut().insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static(" identity "),
    );
    let mut upstream = Upstream {
        responses: VecDeque::from([encoded]),
        ..Default::default()
    };
    assert!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .is_ok()
    );
}

#[test]
async fn validates_all_content_encoding_fields_and_list_members() {
    for (values, accepted) in [
        (vec!["\t iDeNtItY \t", "identity, Identity"], true),
        (vec![", identity,,", "\t, "], true),
        (vec!["identity", "gzip"], false),
        (vec!["gzip", "identity"], false),
        (vec!["identity, gzip"], false),
        (vec!["identity; q=1"], false),
        (vec!["identity identity"], false),
    ] {
        let mut encoded = response(json!({"tools":[]}));
        for value in &values {
            encoded.headers_mut().append(
                header::CONTENT_ENCODING,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let mut upstream = Upstream {
            responses: VecDeque::from([encoded]),
            ..Default::default()
        };
        let result = client(Limits::default()).list_tools(&mut upstream).await;
        if accepted {
            assert!(result.is_ok(), "{values:?}: {result:?}");
        } else {
            assert_eq!(
                result.unwrap_err(),
                protocol("unsupported Content-Encoding"),
                "{values:?}"
            );
        }
    }
}

#[test]
async fn validates_repeated_and_malformed_content_lengths_before_reading_body() {
    for (values, expected) in [
        (vec!["32", "32"], None),
        (vec!["32", "33"], Some(protocol("invalid Content-Length"))),
        (vec!["invalid"], Some(protocol("invalid Content-Length"))),
        (
            vec!["18446744073709551616"],
            Some(protocol("invalid Content-Length")),
        ),
        (vec!["65"], Some(limit("response bytes"))),
    ] {
        let mut encoded = response(json!({"tools":[]}));
        for value in &values {
            encoded.headers_mut().append(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let mut upstream = Upstream {
            responses: VecDeque::from([encoded]),
            ..Default::default()
        };
        let result = client(Limits {
            response_bytes: 64,
            ..Default::default()
        })
        .list_tools(&mut upstream)
        .await;
        match expected {
            None => assert!(result.is_ok(), "{values:?}: {result:?}"),
            Some(error) => assert_eq!(result.unwrap_err(), error, "{values:?}"),
        }
    }
}

#[test]
async fn parses_mime_parameters_and_rejects_ambiguous_content_types() {
    for (values, expected) in [
        (vec!["APPLICATION/JSON; Charset=\"utf-8\""], None),
        (vec!["application/json; extension=\"a;b\""], None),
        (
            vec!["application/json; broken"],
            Some(protocol("expected JSON or SSE Content-Type")),
        ),
        (
            vec!["application/json", "text/event-stream"],
            Some(protocol("multiple Content-Type fields")),
        ),
        (
            vec!["application/json, text/event-stream"],
            Some(protocol("expected JSON or SSE Content-Type")),
        ),
    ] {
        let mut encoded = response(json!({"tools":[]}));
        encoded.headers_mut().remove(header::CONTENT_TYPE);
        for value in &values {
            encoded
                .headers_mut()
                .append(header::CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
        }
        let mut upstream = Upstream {
            responses: VecDeque::from([encoded]),
            ..Default::default()
        };
        let result = client(Limits::default()).list_tools(&mut upstream).await;
        match expected {
            None => assert!(result.is_ok(), "{values:?}: {result:?}"),
            Some(error) => assert_eq!(result.unwrap_err(), error, "{values:?}"),
        }
    }
}

#[test]
async fn compressed_error_responses_preserve_the_http_failure_category() {
    for (status, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            TransportError::AuthorizationRequired(401),
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            TransportError::HttpStatus(503),
        ),
        (StatusCode::OK, protocol("unsupported Content-Encoding")),
    ] {
        let response = Response::builder()
            .status(status)
            .header(header::CONTENT_ENCODING, "gzip")
            .body(body([Bytes::from_static(b"not read")]))
            .unwrap();
        let mut upstream = Upstream {
            responses: VecDeque::from([response]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits::default())
                .list_tools(&mut upstream)
                .await
                .unwrap_err(),
            expected
        );
    }
}

fn sse(data: &str) -> Response<TestBody> {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body(data.as_bytes().chunks(3).map(Bytes::copy_from_slice)))
        .unwrap()
}

#[test]
async fn request_sse_handles_comments_progress_crlf_and_complete_response() {
    let wire = concat!(
        ":keepalive\r\n\r\n",
        "event: ignored\ndata: not-json\n\n",
        "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/extension\"}\n\n",
        "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progressToken\":1,\"progress\":3}}\r\n\r\n",
        "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n",
    );
    let mut upstream = Upstream {
        responses: VecDeque::from([sse(wire)]),
        ..Default::default()
    };
    assert!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .unwrap()
            .tools
            .is_empty()
    );
    assert_eq!(upstream.requests.len(), 1);
    let mut upstream = Upstream {
        responses: VecDeque::from([sse(wire)]),
        ..Default::default()
    };
    assert_eq!(
        client(Limits {
            notifications: 0,
            ..Limits::default()
        })
        .list_tools(&mut upstream)
        .await
        .unwrap_err(),
        limit("notification count")
    );
    let mut upstream = Upstream {
        responses: VecDeque::from([sse("data: {}\n\n")]),
        ..Default::default()
    };
    assert!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .is_err()
    );
    let mut upstream = Upstream {
        responses: VecDeque::from([sse(&format!("data: {}", "x".repeat(200)))]),
        ..Default::default()
    };
    assert_eq!(
        client(Limits {
            response_bytes: 100,
            ..Limits::default()
        })
        .list_tools(&mut upstream)
        .await
        .unwrap_err(),
        limit("response bytes")
    );
}

#[test]
async fn interrupted_json_and_sse_bodies_remain_network_failures() {
    for mime in ["application/json", "text/event-stream"] {
        let broken = StreamBody::new(stream::iter(vec![
            Ok(Frame::data(Bytes::from_static(b" "))),
            Err(TransportError::Network),
        ]))
        .boxed_unsync();
        let mut upstream = Upstream {
            responses: VecDeque::from([Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .body(broken)
                .unwrap()]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits::default())
                .list_tools(&mut upstream)
                .await
                .unwrap_err(),
            TransportError::Network
        );
        assert_eq!(upstream.requests.len(), 1);
    }
    let mut upstream = Upstream {
        responses: VecDeque::from([sse(":keepalive\n\n")]),
        ..Default::default()
    };
    assert_eq!(
        client(Limits::default())
            .list_tools(&mut upstream)
            .await
            .unwrap_err(),
        TransportError::Network
    );
}

#[test]
async fn result_type_defaults_to_complete_but_never_drives_continuations() {
    for result in [
        json!({"content":[]}),
        json!({"content":[], "resultType":"complete"}),
    ] {
        let mut upstream = Upstream {
            responses: VecDeque::from([response(result.clone())]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits::default())
                .call_tool(&mut upstream, &tool(), &input(), None)
                .await
                .unwrap(),
            result
        );
    }
    for kind in ["input_required", "task"] {
        let mut upstream = Upstream {
            responses: VecDeque::from([response(json!({"resultType":kind}))]),
            ..Default::default()
        };
        assert_eq!(
            client(Limits::default())
                .call_tool(&mut upstream, &tool(), &input(), None)
                .await
                .unwrap_err(),
            TransportError::UnsupportedCapability
        );
        assert_eq!(upstream.requests.len(), 1);
    }
}

struct DropSignal(Arc<AtomicBool>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
#[test_r::timeout("10s")]
async fn timeout_drops_the_stream_and_releases_the_shared_permit() {
    let dropped = Arc::new(AtomicBool::new(false));
    let signal = DropSignal(dropped.clone());
    let stream: BoxStream<'static, Result<Frame<Bytes>, TransportError>> =
        stream::once(async move {
            let _signal = signal;
            futures::future::pending().await
        })
        .boxed();
    let stalled = Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(StreamBody::new(stream).boxed_unsync())
        .unwrap();
    let mut upstream = Upstream {
        responses: VecDeque::from([stalled, numbered_response(2, json!({"tools":[]}))]),
        ..Default::default()
    };
    let client = client(Limits {
        timeout: Duration::from_millis(25),
        concurrency: 1,
        ..Limits::default()
    });
    assert_eq!(
        client.list_tools(&mut upstream).await.unwrap_err(),
        TransportError::Timeout
    );
    assert!(dropped.load(Ordering::SeqCst));
    assert!(client.clone().list_tools(&mut upstream).await.is_ok());
    assert_eq!(upstream.requests.len(), 2);
}

#[test]
async fn validates_configuration_and_separates_credentials() {
    assert!(
        Client::new(
            "https://tools.example",
            Some("2025-11-25"),
            Limits::default()
        )
        .is_err()
    );
    for credential in ["", "secret\r\nInjected: yes", "a=b", "a b"] {
        let error = client(Limits::default())
            .with_bearer(credential)
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret"));
    }
    let basic = client(Limits::default())
        .with_basic("alice", "pass:word")
        .unwrap();
    let bearer = client(Limits::default()).with_bearer("bob-token").unwrap();
    let mut upstream = Upstream {
        responses: VecDeque::from([response(json!({"tools":[]})), response(json!({"tools":[]}))]),
        ..Default::default()
    };
    basic.list_tools(&mut upstream).await.unwrap();
    bearer.list_tools(&mut upstream).await.unwrap();
    assert_eq!(
        upstream.requests[0].headers()[header::AUTHORIZATION],
        "Basic YWxpY2U6cGFzczp3b3Jk"
    );
    assert_eq!(
        upstream.requests[1].headers()[header::AUTHORIZATION],
        "Bearer bob-token"
    );
}

#[test]
async fn oauth_probe_is_unauthenticated_bounded_and_does_not_poll_response_body() {
    let challenges = [
        "Basic realm=other",
        "Bearer resource_metadata=\"https://tools.example/meta\"",
    ];
    let size: usize = challenges.iter().map(|s| s.len()).sum();
    for maximum in [size - 1, size] {
        let unread_body = StreamBody::new(stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Frame<Bytes>, TransportError>>> {
                panic!("OAuth probe must not poll a response body")
            },
        ))
        .boxed_unsync();
        let mut response = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("set-cookie", "private-cookie")
            .body(unread_body)
            .unwrap();
        for value in challenges {
            response
                .headers_mut()
                .append(header::WWW_AUTHENTICATE, HeaderValue::from_static(value));
        }
        let mut upstream = Upstream {
            responses: VecDeque::from([response]),
            ..Default::default()
        };
        let client = client(Limits::default())
            .with_bearer("private-credential")
            .unwrap();
        let result = client.authorization_challenge(&mut upstream, maximum).await;
        if maximum == size {
            let headers = result.unwrap();
            assert!(!headers.contains_key("set-cookie"));
            assert_eq!(
                headers
                    .get_all(header::WWW_AUTHENTICATE)
                    .iter()
                    .map(|h| h.to_str().unwrap())
                    .collect::<Vec<_>>(),
                challenges
            );
        } else {
            assert_eq!(result.unwrap_err(), limit("OAuth challenge bytes"));
        }
        assert_eq!(upstream.requests.len(), 1);
        let request = &upstream.requests[0];
        assert_eq!(request.method(), http::Method::POST);
        assert_eq!(request.headers()["Mcp-Method"], "tools/list");
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        assert!(!request.headers().contains_key("cookie"));
        let payload: Value = serde_json::from_slice(request.body()).unwrap();
        assert_eq!(payload["method"], "tools/list");
        assert_eq!(
            payload["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            PROTOCOL_VERSION
        );
    }
}

#[test]
async fn oauth_probe_only_uses_401_challenges_and_never_retries_status_or_admission_errors() {
    for status in [
        StatusCode::OK,
        StatusCode::FOUND,
        StatusCode::FORBIDDEN,
        StatusCode::BAD_GATEWAY,
    ] {
        let mut response = raw_json("not a tool observation".into(), status);
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=ignored"),
        );
        let mut upstream = Upstream {
            responses: VecDeque::from([response]),
            ..Default::default()
        };
        let result = client(Limits::default())
            .authorization_challenge(&mut upstream, 0)
            .await;
        if status == StatusCode::OK {
            assert!(result.unwrap().is_empty());
        } else {
            assert_eq!(
                result.unwrap_err(),
                TransportError::HttpStatus(status.as_u16())
            );
        }
        assert_eq!(upstream.requests.len(), 1);
    }
    let mut upstream = Upstream {
        rejection: Some(TransportError::QuotaExhausted),
        ..Default::default()
    };
    assert_eq!(
        client(Limits::default())
            .authorization_challenge(&mut upstream, 100)
            .await
            .unwrap_err(),
        TransportError::QuotaExhausted
    );
    assert_eq!(upstream.requests.len(), 1);
}
