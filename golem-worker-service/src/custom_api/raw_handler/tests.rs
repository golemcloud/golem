use super::*;
use golem_schema::proto::golem::schema::{
    ListValue, RecordValue, SchemaValueStreamReference, schema_value,
};
use test_r::test;

fn value(value: schema_value::Value) -> ProtoValue {
    ProtoValue { value: Some(value) }
}

fn bytes_value(bytes: &[u8]) -> ProtoValue {
    Vec::from(bytes).to_value().try_into().unwrap()
}

#[test]
#[test_r::timeout("10s")]
async fn opening_size_error_returns_413_with_the_original_message() {
    use crate::api::common::ApiEndpointError;
    use crate::custom_api::http_session::tests::harness;
    use crate::service::worker::{CallWorkerExecutorError, WorkerServiceError};
    use poem::IntoResponse;

    for stream_error in [false, true] {
        let h = harness(1);
        let status = tonic::Status::out_of_range(
            "Error, decoded message length too large: found 2048 bytes, the limit is: 1024 bytes",
        );
        if stream_error {
            h.response_txs[0].send(Err(status.clone())).await.unwrap();
        } else {
            h.reject_start(WorkerServiceError::InternalCallError(
                CallWorkerExecutorError::MessageTooLarge(status.clone()),
            ))
            .await;
        }
        let mut session = h.start_session();
        let error = receive_response_head(
            &mut session,
            poem::Body::empty(),
            &mut None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();
        let response = ApiEndpointError::from(error).into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body: serde_json::Value = response.into_body().into_json().await.unwrap();
        assert_eq!(body["code"], "REQUEST_PAYLOAD_TOO_LARGE");
        assert!(body["error"].as_str().unwrap().contains(status.message()));
    }
}

#[test]
#[test_r::timeout("10s")]
async fn late_upload_overflow_cancels_the_session_without_buffering_the_body() {
    use crate::custom_api::http_session::tests::{accepted, harness, stream_result};
    use golem_service_base::body_limit::RequestBodyLimit;

    let h = harness(1);
    let mut session = h.start_session();
    let (upload_tx, upload_rx) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(2);
    let mut request = poem::Request::builder().body(poem::Body::from_bytes_stream(
        tokio_stream::wrappers::ReceiverStream::new(upload_rx),
    ));
    let limit = RequestBodyLimit::new(16).apply(&mut request);
    let mut upload = None;
    let done = Arc::new(AtomicBool::new(false));
    h.response_txs[0].send(Ok(accepted(1, None))).await.unwrap();
    h.response_txs[0].send(Ok(stream_result())).await.unwrap();
    let _head = receive_response_head(&mut session, request.take_body(), &mut upload, done.clone())
        .await
        .unwrap();
    // Response headers are available while the request body has not supplied a single chunk.
    assert!(limit.rejection().is_none());
    let response_head = process_response_head("POST", 200, &vec![]).unwrap();
    let guard = session.drop_guard();
    let response =
        poem::Response::builder()
            .status(StatusCode::OK)
            .body(poem::Body::from_bytes_stream(futures::stream::unfold(
                OutputState {
                    session,
                    upload,
                    guard: Some(guard),
                    gate: BodyGate::new(&response_head),
                    output_id: 9,
                    terminal: false,
                },
                output_step,
            )));
    upload_tx
        .send(Ok(Bytes::from_static(b"0123456789abcdef")))
        .await
        .unwrap();
    upload_tx.send(Ok(Bytes::from_static(b"x"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        response.into_body().into_vec(),
    )
    .await
    .expect("overflow must cancel the session without waiting for upload EOF");
    assert!(body.is_err());
    assert!(limit.rejection().is_some());
    assert!(!done.load(Ordering::Acquire));
    drop(upload_tx);
}

#[test]
fn canonical_byte_lists_are_not_packed() {
    let encoded = bytes_value(&[0, 127, 255]);
    let schema_value::Value::ListValue(list) = encoded.value.unwrap() else {
        panic!("not a list")
    };
    assert_eq!(list.elements.len(), 3);
    assert_eq!(
        decode_bytes(&value(schema_value::Value::ListValue(list))).unwrap(),
        [0, 127, 255]
    );
}

#[test]
fn strict_response_head_decoder_accepts_canonical_shape() {
    let header = value(schema_value::Value::RecordValue(RecordValue {
        fields: vec![
            value(schema_value::Value::StringValue("x-test".into())),
            bytes_value(b"yes"),
        ],
    }));
    let response = value(schema_value::Value::RecordValue(RecordValue {
        fields: vec![
            value(schema_value::Value::U16Value(201)),
            value(schema_value::Value::ListValue(ListValue {
                elements: vec![header],
            })),
            value(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 9 },
            )),
        ],
    }));
    assert_eq!(
        parse_response(response).unwrap(),
        (201, vec![(b"x-test".to_vec(), b"yes".to_vec())], 9)
    );
}

#[test]
fn response_decoder_rejects_non_byte_header_values() {
    let header = value(schema_value::Value::RecordValue(RecordValue {
        fields: vec![
            value(schema_value::Value::StringValue("x".into())),
            value(schema_value::Value::StringValue("not bytes".into())),
        ],
    }));
    let response = value(schema_value::Value::RecordValue(RecordValue {
        fields: vec![
            value(schema_value::Value::U16Value(200)),
            value(schema_value::Value::ListValue(ListValue {
                elements: vec![header],
            })),
            value(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 1 },
            )),
        ],
    }));
    assert!(parse_response(response).is_err());
}

#[test]
fn response_decoder_rejects_overflow_and_extra_header_fields() {
    let name = value(schema_value::Value::StringValue("x-test".into()));
    let bytes = bytes_value(b"yes");
    for (status, header_fields) in [
        (65_536, vec![name.clone(), bytes.clone()]),
        (200, vec![name.clone(), bytes.clone(), bytes.clone()]),
        (
            200,
            vec![
                name,
                value(schema_value::Value::ListValue(ListValue {
                    elements: vec![value(schema_value::Value::U8Value(256))],
                })),
            ],
        ),
    ] {
        let response = value(schema_value::Value::RecordValue(RecordValue {
            fields: vec![
                value(schema_value::Value::U16Value(status)),
                value(schema_value::Value::ListValue(ListValue {
                    elements: vec![value(schema_value::Value::RecordValue(RecordValue {
                        fields: header_fields,
                    }))],
                })),
                value(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 9 },
                )),
            ],
        }));
        assert!(parse_response(response).is_err());
    }
}

#[test]
#[test_r::timeout("10s")]
async fn late_stream_error_never_writes_successful_chunked_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let wait = Arc::new(tokio::sync::Mutex::new(Some(wait)));
    let endpoint = poem::endpoint::make(move |_| {
        let wait = wait.clone();
        async move {
            let wait = wait.lock().await.take().unwrap();
            let chunks = futures::stream::once(async {
                Ok::<_, io::Error>(Bytes::from_static(b"before-error"))
            })
            .chain(futures::stream::once(async move {
                wait.await.unwrap();
                Err(io::Error::other("late failure"))
            }));
            poem::Response::builder().body(poem::Body::from_bytes_stream(chunks))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
    let _server = AbortOnDropHandle::new(tokio::spawn(golem_service_base::http_server::run(
        acceptor,
        endpoint,
        Default::default(),
    )));
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut wire = Vec::new();
    let mut buffer = [0; 1024];
    while !wire.ends_with(b"before-error\r\n") {
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0, "closed before first response chunk");
        wire.extend_from_slice(&buffer[..count]);
    }
    release.send(()).unwrap();
    // Either a reset or an incomplete EOF signals failure; a zero chunk does not.
    let _ = socket.read_to_end(&mut wire).await;
    assert!(
        wire.windows(b"transfer-encoding: chunked".len())
            .any(|part| part == b"transfer-encoding: chunked")
    );
    assert!(
        !wire.ends_with(b"0\r\n\r\n"),
        "late failure emitted successful EOF: {wire:?}"
    );
}

#[test]
#[test_r::timeout("10s")]
async fn h2_reset_drops_backpressured_response_body() {
    struct DropProbe(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let (dropped, wait_drop) = tokio::sync::oneshot::channel();
    let probe = Arc::new(tokio::sync::Mutex::new(Some(DropProbe(Some(dropped)))));
    let endpoint = poem::endpoint::make(move |_| {
        let probe = probe.clone();
        async move {
            let probe = probe.lock().await.take().unwrap();
            let chunks = futures::stream::unfold(probe, |probe| async move {
                Some((Ok::<_, io::Error>(Bytes::from(vec![0xab; 65536])), probe))
            });
            poem::Response::builder().body(poem::Body::from_bytes_stream(chunks))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
    let _server = AbortOnDropHandle::new(tokio::spawn(golem_service_base::http_server::run(
        acceptor,
        endpoint,
        Default::default(),
    )));
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (mut client, connection) = h2::client::handshake(socket).await.unwrap();
    let _connection = AbortOnDropHandle::new(tokio::spawn(connection));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://{address}/"))
        .body(())
        .unwrap();
    let (response, mut request_body) = client.send_request(request, false).unwrap();
    request_body
        .send_data(Bytes::from_static(b"x"), true)
        .unwrap();
    let response = response.await.unwrap();
    request_body.send_reset(h2::Reason::CANCEL);
    drop(response);
    tokio::time::timeout(std::time::Duration::from_secs(3), wait_drop)
        .await
        .unwrap()
        .unwrap();
}

async fn raw_status(request: &'static [u8]) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let endpoint = poem::endpoint::make(|_| async {
        poem::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .finish()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
    let _server = AbortOnDropHandle::new(tokio::spawn(golem_service_base::http_server::run(
        acceptor,
        endpoint,
        Default::default(),
    )));
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(request).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let status = response
        .split(|byte| *byte == b' ')
        .nth(1)
        .expect("HTTP response status");
    std::str::from_utf8(status).unwrap().parse().unwrap()
}

#[test]
#[test_r::timeout("10s")]
async fn official_transport_accepts_equal_duplicate_content_length() {
    assert_eq!(
            raw_status(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .await,
            StatusCode::NO_CONTENT.as_u16()
        );
}

#[test]
#[test_r::timeout("10s")]
async fn official_transport_rejects_unsupported_bare_transfer_encoding() {
    assert_eq!(
            raw_status(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip\r\nConnection: close\r\n\r\n"
            )
            .await,
            StatusCode::BAD_REQUEST.as_u16()
        );
}
