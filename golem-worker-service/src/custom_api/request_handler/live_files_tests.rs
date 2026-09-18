use super::*;
use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
use crate::service::worker::{WorkerResult, WorkerServiceError};
use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use golem_common::model::agent::FileMapping;
use golem_common::model::filesystem::{
    FileByteSelection, FileReadError, FileReadExtent, FileReadHead, FileReadMetadata,
};
use golem_service_base::custom_api::{ConstructorParameter, PathSegmentType};
use golem_service_base::model::FileReadResponse;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use test_r::test;

fn route(mappings: &[(&str, &str)]) -> CompiledRoute {
    let mut route = test_route(1, "/sites/{id}", None, "filesystem");
    route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
    let RouteBehaviour::AgentFilesystem(filesystem) = &mut route.behavior else {
        unreachable!()
    };
    filesystem.constructor_input = CompiledInputSchema {
        graph: SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "id".into(),
                body: SchemaType::u64(),
                metadata: Default::default(),
            },
        ])),
        input_schema: InputSchema::Parameters(vec![
            golem_common::schema::NamedField::user_supplied("id", SchemaType::u64()),
        ]),
    };
    filesystem.constructor_parameters = vec![ConstructorParameter::Path {
        path_segment_index: golem_service_base::model::SafeIndex::new(0),
        parameter_type: PathSegmentType::U64,
    }];
    filesystem.filesystem_bindings = FileMapping::compile_list(mappings.iter().copied()).unwrap();
    route
}

fn endpoint(harness: &InvocationHarness, routes: Vec<CompiledRoute>) -> CustomApiPoemEndpoint {
    endpoint_with_timeout(harness, routes, std::time::Duration::from_secs(300))
}

fn endpoint_with_timeout(
    harness: &InvocationHarness,
    routes: Vec<CompiledRoute>,
    timeout: std::time::Duration,
) -> CustomApiPoemEndpoint {
    let files = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let mut handler = request_handler_with_worker(
        test_resolver(routes),
        files.clone(),
        harness.worker_service.clone(),
    );
    handler.raw_handler = RawHandler::new(
        harness.worker_service.clone(),
        crate::config::HttpSessionLimits {
            exchange_timeout: timeout,
            ..Default::default()
        },
        files,
    );
    CustomApiPoemEndpoint::new(Arc::new(handler))
}

fn request(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
    let mut request = Request::builder()
        .method(method.parse().unwrap())
        .uri(path.parse().unwrap())
        .header("host", "example.com")
        .header("origin", "https://client.example");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.finish()
}

fn enqueue(harness: &InvocationHarness, response: WorkerResult<FileReadResponse>) {
    harness
        .file_reads
        .responses
        .lock()
        .unwrap()
        .push_back(std::future::ready(response).boxed());
}

fn read(head: FileReadHead, bytes: Vec<u8>) -> FileReadResponse {
    let body = if bytes.is_empty() {
        // Bodyless responses must dispose the gRPC body, not poll it to infer metadata.
        futures::stream::poll_fn(|_| panic!("bodyless file stream polled")).boxed()
    } else {
        futures::stream::iter(
            bytes
                .chunks(3)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>(),
        )
        .boxed()
    };
    FileReadResponse { head, body }
}

fn file(total: u64, offset: u64, length: u64, bytes: &[u8]) -> FileReadResponse {
    read(
        FileReadHead::File(FileReadMetadata {
            total_size: total,
            selection: FileReadExtent::Selected { offset, length },
            modified_at: None,
        }),
        bytes.to_vec(),
    )
}

fn decode(value: &serde_json::Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
#[test_r::timeout("30s")]
async fn live_http_wire_deadlines_disconnects_and_late_failure() {
    use std::time::Duration;
    use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

    for http2 in [false, true] {
        for scenario in [
            "head-deadline",
            "head-disconnect",
            "body-deadline",
            "body-disconnect",
            "body-error",
            "unread-upload",
        ] {
            let harness = invocation_harness();
            let cancelled = CancellationToken::new();
            let guard = cancelled.clone().drop_guard();
            let entered = Arc::new(tokio::sync::Notify::new());
            let started = entered.clone();
            let fail = Arc::new(tokio::sync::Notify::new());
            let trigger = fail.clone();
            harness.file_reads.responses.lock().unwrap().push_back(
                async move {
                    started.notify_one();
                    if scenario.starts_with("head-") {
                        let _guard = guard;
                        std::future::pending().await
                    } else if scenario == "unread-upload" {
                        drop(guard);
                        Ok(file(3, 0, 3, b"abc"))
                    } else {
                        Ok(FileReadResponse {
                            head: file(3, 0, 3, b"abc").head,
                            body: futures::stream::once(async { Ok(Bytes::from_static(b"abc")) })
                                .chain(futures::stream::once(async move {
                                    let _guard = guard;
                                    fail.notified().await;
                                    Err(FileReadError::Lifecycle)
                                }))
                                .boxed(),
                        })
                    }
                }
                .boxed(),
            );
            // H1 may not notice disconnect until the pending handler's deadline.
            let deadline =
                if scenario.ends_with("deadline") || (!http2 && scenario == "head-disconnect") {
                    Duration::from_secs(1)
                } else {
                    Duration::from_secs(10)
                };
            let endpoint = Arc::new(endpoint_with_timeout(
                &harness,
                vec![route(&[("/*", "/a/$1")])],
                deadline,
            ));
            let endpoint = poem::endpoint::make(move |request| {
                let endpoint = endpoint.clone();
                async move { endpoint.execute(request).await }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
            let _server = AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(
                acceptor, endpoint,
            )));
            let builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
            let client = if http2 {
                builder.http2_prior_knowledge()
            } else {
                builder.http1_only()
            }
            .build()
            .unwrap();
            let response = tokio::spawn(async move {
                let mut request = client.get(format!("http://{address}/sites/42/a"));
                if scenario == "unread-upload" {
                    request = request.body(reqwest::Body::wrap_stream(
                        futures::stream::once(async {
                            Ok::<_, std::io::Error>(Bytes::from_static(b"unfinished upload"))
                        })
                        .chain(futures::stream::pending()),
                    ));
                }
                request.send().await
            });
            tokio::time::timeout(Duration::from_secs(3), entered.notified())
                .await
                .unwrap_or_else(|_| panic!("{http2} {scenario}: file read not started"));
            if scenario == "head-disconnect" {
                response.abort();
                let _ = response.await;
            } else {
                let mut response = response.await.unwrap().unwrap();
                if scenario == "head-deadline" {
                    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
                } else if scenario == "unread-upload" {
                    assert_eq!(response.status(), StatusCode::OK);
                    assert_eq!(response.bytes().await.unwrap(), b"abc".as_slice());
                } else {
                    assert_eq!(response.status(), StatusCode::OK);
                    assert_eq!(response.chunk().await.unwrap().unwrap(), b"ab".as_slice());
                    if scenario == "body-disconnect" {
                        drop(response);
                    } else {
                        if scenario == "body-error" {
                            trigger.notify_one();
                        }
                        assert!(
                            response.bytes().await.is_err(),
                            "{http2} {scenario}: false successful EOF"
                        );
                    }
                }
            }
            tokio::time::timeout(Duration::from_secs(2), cancelled.cancelled())
                .await
                .unwrap_or_else(|_| panic!("{http2} {scenario}: source retained"));
            assert_eq!(harness.file_reads.calls.lock().unwrap().len(), 1);
        }
    }
}

#[test]
async fn mounted_read_rejects_paths_before_normalization() {
    let harness = invocation_harness();
    let agent = golem_common::model::AgentId {
        component_id: harness.component_id,
        agent_id: "agent1(42)".into(),
    };
    for path in [
        "/public/../private",
        "/public//a",
        "/public/./a",
        "public/a",
        "/public/a/",
    ] {
        assert!(matches!(
            harness
                .worker_service
                .read_mounted_file(
                    &agent,
                    path,
                    FileByteSelection::Full,
                    harness.environment_id,
                    harness.account_id,
                )
                .await,
            Err(WorkerServiceError::FileRead(FileReadError::InvalidTarget))
        ));
    }
    assert!(harness.file_reads.calls.lock().unwrap().is_empty());
}

#[test]
fn live_deadline_is_shared_by_mappings_head_and_body() {
    use std::time::Duration;
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::pause();
            let harness = invocation_harness();
            let cancelled = CancellationToken::new();
            for head in [FileReadHead::Absent, file(3, 0, 3, b"abc").head] {
                let guard = cancelled.clone().drop_guard();
                harness.file_reads.responses.lock().unwrap().push_back(
                    async move {
                        let _guard = guard;
                        tokio::time::sleep(Duration::from_secs(200)).await;
                        _guard.disarm();
                        Ok(read(head, vec![]))
                    }
                    .boxed(),
                );
            }
            let start = Instant::now();
            let response = endpoint(&harness, vec![route(&[("/*", "/a/$1"), ("/*", "/b/$1")])])
                .execute(request("GET", "/sites/42/file", &[]))
                .await;
            assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(
                response.headers()["access-control-allow-origin"],
                "https://client.example"
            );
            assert!(start.elapsed() >= Duration::from_secs(300));
            assert!(start.elapsed() < Duration::from_secs(301));
            assert_eq!(harness.file_reads.calls.lock().unwrap().len(), 2);
            assert!(cancelled.is_cancelled());

            let harness = invocation_harness();
            let cancelled = CancellationToken::new();
            let guard = cancelled.clone().drop_guard();
            let source = futures::stream::poll_fn(move |_| {
                let _ = &guard;
                std::task::Poll::Pending
            })
            .boxed();
            harness.file_reads.responses.lock().unwrap().push_back(
                async move {
                    tokio::time::sleep(Duration::from_secs(200)).await;
                    Ok(FileReadResponse {
                        head: file(3, 0, 3, b"abc").head,
                        body: source,
                    })
                }
                .boxed(),
            );
            let start = Instant::now();
            let mut response = endpoint(&harness, vec![route(&[("/*", "/a/$1")])])
                .execute(request("GET", "/sites/42/file", &[]))
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(start.elapsed() >= Duration::from_secs(200));
            assert!(start.elapsed() < Duration::from_secs(201));
            assert!(!cancelled.is_cancelled());
            tokio::time::advance(Duration::from_secs(100)).await;
            tokio::task::yield_now().await;
            assert!(cancelled.is_cancelled());
            assert!(response.take_body().into_vec().await.is_err());
        });
}

#[test]
async fn mounted_read_auth_uses_system_and_typed_selection_is_terminal() {
    let harness = invocation_harness();
    let mut protected = route(&[("/*", "/public/$1")]);
    protected.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
        header_name: "x-session".into(),
    });
    let mut typed = test_route(2, "/sites/{id}/typed", Some("GET"), "typed");
    typed.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
        header_name: "x-typed-session".into(),
    });
    let endpoint = endpoint(&harness, vec![protected, typed]);
    assert_eq!(
        endpoint
            .execute(request("GET", "/sites/42/typed", &[("x-session", "{}")]))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for id in [42, 9] {
        enqueue(&harness, Ok(file(1, 0, 1, b"x")));
        let response = endpoint
            .execute(request(
                "GET",
                &format!("/sites/{id}/a"),
                &[("x-session", "{\"subject\":\"not-an-api-token\"}")],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.into_body().into_vec().await.unwrap(), b"x");
    }
    let calls = harness.file_reads.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].agent_id.agent_id, "agent1(42)");
    assert_eq!(calls[1].agent_id.agent_id, "agent1(9)");
    assert!(
        calls
            .iter()
            .all(|call| matches!(call.auth, AuthCtx::System))
    );
}

#[test]
async fn live_file_representation_corpus_through_request_handler() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let mut tested = 0;
    for case in corpus["cases"].as_array().unwrap().iter().filter(|case| {
        case["suite"] == "files"
            && case["input"]["mode"] == "live"
            && case["input"]["body_hex"].is_string()
    }) {
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let expect = &case["expect"];
        let bytes = decode(&input["body_hex"]);
        let expected = decode(&expect["body_hex"]);
        let status = expect["status"].as_u64().unwrap();
        let metadata_only = input["method"] == "HEAD" || status == 304;
        let extent = if status == 416 {
            FileReadExtent::Unsatisfiable
        } else if metadata_only {
            FileReadExtent::Selected {
                offset: 0,
                length: 0,
            }
        } else {
            let offset = if status == 206 {
                (bytes.len() - expected.len()) as u64
            } else {
                0
            };
            FileReadExtent::Selected {
                offset,
                length: expected.len() as u64,
            }
        };
        let harness = invocation_harness();
        enqueue(
            &harness,
            Ok(read(
                FileReadHead::File(FileReadMetadata {
                    total_size: bytes.len() as u64,
                    selection: extent,
                    modified_at: None,
                }),
                expected.clone(),
            )),
        );
        let endpoint = endpoint(&harness, vec![route(&[("/*", "/public/$1")])]);
        let headers: Vec<_> = input["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h[0].as_str().unwrap(), h[1].as_str().unwrap()))
            .collect();
        let response = endpoint
            .execute(request(
                input["method"].as_str().unwrap(),
                &format!("/sites/42{}", input["path"].as_str().unwrap()),
                &headers,
            ))
            .await;
        assert_eq!(u64::from(response.status().as_u16()), status, "{id}");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://client.example",
            "{id}"
        );
        for header in expect["headers"].as_array().into_iter().flatten() {
            assert_eq!(
                response.headers().get(header[0].as_str().unwrap()).unwrap(),
                header[1].as_str().unwrap(),
                "{id}"
            );
        }
        for header in expect["absent_headers"].as_array().into_iter().flatten() {
            assert!(
                !response.headers().contains_key(header.as_str().unwrap()),
                "{id}"
            );
        }
        assert_eq!(
            response.into_body().into_vec().await.unwrap(),
            expected,
            "{id}"
        );
        let calls = harness.file_reads.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{id}");
        assert_eq!(calls[0].agent_id.agent_id, "agent1(42)");
        assert_eq!(
            calls[0].path,
            format!("/public{}", input["path"].as_str().unwrap())
        );
        assert_eq!(calls[0].environment_id, EnvironmentId(uuid::Uuid::nil()));
        assert_eq!(calls[0].account_id, AccountId(uuid::Uuid::nil()));
        assert!(matches!(calls[0].auth, AuthCtx::System));
        if metadata_only {
            assert_eq!(calls[0].selection, FileByteSelection::MetadataOnly, "{id}");
        }
        if id == "file-range-open-ended" {
            assert_eq!(
                calls[0].selection,
                FileByteSelection::OpenEnded { start: 7 }
            );
        }
        if id == "file-range-suffix" {
            assert_eq!(calls[0].selection, FileByteSelection::Suffix { length: 4 });
        }
        tested += 1;
    }
    assert_eq!(tested, 10);
}

#[test]
async fn live_file_ranges_and_preconditions() {
    for (headers, selection, head, status, expected) in [
        (
            vec![("range", "bytes=2-4")],
            FileByteSelection::Bounded {
                start: 2,
                end_inclusive: 4,
            },
            file(10, 2, 3, b"234"),
            206,
            b"234".as_slice(),
        ),
        (
            vec![("range", "bytes=0-0,2-2")],
            FileByteSelection::Full,
            file(3, 0, 3, b"abc"),
            200,
            b"abc".as_slice(),
        ),
        (
            vec![("range", "bytes=2-1")],
            FileByteSelection::Full,
            file(3, 0, 3, b"abc"),
            200,
            b"abc".as_slice(),
        ),
        (
            vec![("range", "bytes=1-"), ("range", "bytes=2-")],
            FileByteSelection::Full,
            file(3, 0, 3, b"abc"),
            200,
            b"abc".as_slice(),
        ),
        (
            vec![("if-match", "*")],
            FileByteSelection::Full,
            file(3, 0, 3, b"abc"),
            200,
            b"abc".as_slice(),
        ),
        (
            vec![("if-match", "\"tag\""), ("if-none-match", "*")],
            FileByteSelection::MetadataOnly,
            file(3, 0, 0, b""),
            412,
            b"".as_slice(),
        ),
        (
            vec![("if-none-match", "invalid")],
            FileByteSelection::MetadataOnly,
            file(3, 0, 0, b""),
            400,
            b"".as_slice(),
        ),
    ] {
        let harness = invocation_harness();
        enqueue(&harness, Ok(head));
        let response = endpoint(&harness, vec![route(&[("/*", "/public/$1")])])
            .execute(request("GET", "/sites/8/a.txt", &headers))
            .await;
        assert_eq!(response.status().as_u16(), status, "{headers:?}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(!response.headers().contains_key("etag"));
        assert!(!response.headers().contains_key("last-modified"));
        if status != 400 {
            assert_eq!(response.into_body().into_vec().await.unwrap(), expected);
        }
        assert_eq!(
            harness.file_reads.calls.lock().unwrap()[0].selection,
            selection
        );
    }
}

#[test]
async fn live_file_only_absence_advances_and_errors_are_private() {
    for (first, status, calls) in [
        (Ok(read(FileReadHead::Absent, vec![])), 200, 2),
        (Ok(read(FileReadHead::Symlink, vec![])), 403, 1),
        (Ok(read(FileReadHead::PermissionDenied, vec![])), 403, 1),
        (Ok(read(FileReadHead::NotRegular, vec![])), 403, 1),
        (
            Err(WorkerServiceError::FileRead(
                FileReadError::ResourceExhausted,
            )),
            503,
            1,
        ),
        (
            Err(WorkerServiceError::Internal(
                "/private/secret/agent-id".into(),
            )),
            500,
            1,
        ),
        (
            Err(WorkerServiceError::FileRead(FileReadError::Lifecycle)),
            500,
            1,
        ),
    ] {
        let harness = invocation_harness();
        enqueue(&harness, first);
        enqueue(&harness, Ok(file(3, 0, 3, b"abc")));
        let response = endpoint(
            &harness,
            vec![route(&[("/*", "/public/$1"), ("/*", "/backup/$1")])],
        )
        .execute(request("GET", "/sites/42/a", &[]))
        .await;
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://client.example"
        );
        let body = response.into_body().into_vec().await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("/private"));
        let actual = harness.file_reads.calls.lock().unwrap();
        assert_eq!(actual.len(), calls);
        assert_eq!(actual[0].path, "/public/a");
        if calls == 2 {
            assert_eq!(actual[1].path, "/backup/a");
            assert_eq!(body, b"abc");
        }
    }
    for headers in [
        vec![("if-none-match", "*")],
        vec![("if-none-match", "malformed")],
    ] {
        let harness = invocation_harness();
        enqueue(&harness, Ok(read(FileReadHead::Absent, vec![])));
        enqueue(&harness, Ok(read(FileReadHead::Absent, vec![])));
        let response = endpoint(
            &harness,
            vec![route(&[("/*", "/public/$1"), ("/*", "/backup/$1")])],
        )
        .execute(request("GET", "/sites/42/a", &headers))
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(harness.file_reads.calls.lock().unwrap().len(), 2);
    }
}

#[test]
async fn live_file_directory_intent_and_literal_decoding() {
    for (path, target, directory) in [
        ("/sites/9/%252e%252e", "/public/%2e%2e", false),
        ("/sites/9/a/", "/public/a", true),
        ("/sites/9/", "/public", false),
    ] {
        let harness = invocation_harness();
        enqueue(
            &harness,
            Ok(if directory {
                file(3, 0, 0, b"")
            } else {
                file(3, 0, 3, b"abc")
            }),
        );
        let response = endpoint(&harness, vec![route(&[("/*", "/public/$1")])])
            .execute(request("GET", path, &[]))
            .await;
        assert_eq!(
            response.status().as_u16(),
            if directory { 403 } else { 200 }
        );
        let calls = harness.file_reads.calls.lock().unwrap();
        assert_eq!(calls[0].path, target);
        assert_eq!(
            calls[0].selection,
            if directory {
                FileByteSelection::MetadataOnly
            } else {
                FileByteSelection::Full
            }
        );
    }
}

#[test]
async fn live_file_auth_preflight_and_invalid_requests_do_not_read() {
    let harness = invocation_harness();
    let mut protected = route(&[("/*", "/public/$1")]);
    protected.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
        header_name: "x-session".into(),
    });
    let endpoint = endpoint(&harness, vec![protected]);
    for (method, path, headers, status) in [
        ("GET", "/sites/42/a", vec![], 401),
        ("POST", "/sites/42/a", vec![], 404),
        ("GET", "/sites/42/%2e%2e/a", vec![], 400),
        ("GET", "/sites/invalid/a", vec![("x-session", "{}")], 400),
        (
            "OPTIONS",
            "/sites/42/a",
            vec![("access-control-request-method", "GET")],
            204,
        ),
    ] {
        assert_eq!(
            endpoint
                .execute(request(method, path, &headers))
                .await
                .status()
                .as_u16(),
            status,
            "{method} {path}"
        );
    }
    assert!(harness.file_reads.calls.lock().unwrap().is_empty());
}
