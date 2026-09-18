use super::*;
use futures::TryStreamExt;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::sync::Arc;
use test_r::test;

fn headers(values: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in values {
        headers.append(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

#[test]
fn ranges_and_preconditions() {
    for (values, size, expected) in [
        (vec![("range", "bytes=2-5")], 10, (206, (2, 4))),
        (vec![("range", "bytes=7-")], 10, (206, (7, 3))),
        (vec![("range", "bytes=-3")], 10, (206, (7, 3))),
        (vec![("range", "bytes=-90")], 10, (206, (0, 10))),
        (vec![("range", "bytes=8-90")], 10, (206, (8, 2))),
        (vec![("range", "bytes=0-0")], 0, (416, (0, 0))),
        (vec![("range", "bytes=-1")], 0, (416, (0, 0))),
        (vec![("range", "bytes=-0")], 10, (416, (0, 0))),
        (vec![("range", "bytes=10-")], 10, (416, (0, 0))),
        (vec![("range", "bytes=9-8")], 10, (200, (0, 10))),
        (vec![("range", "bytes=0-0,2-2")], 10, (200, (0, 10))),
        (
            vec![("range", "bytes=0-0"), ("range", "bytes=2-2")],
            10,
            (200, (0, 10)),
        ),
        (
            vec![("range", "bytes=0-18446744073709551616")],
            10,
            (200, (0, 10)),
        ),
        (vec![("range", "bytes=+1-2")], 10, (200, (0, 10))),
        (vec![("range", "items=0-2")], 10, (200, (0, 10))),
        (vec![("range", "bytes=0-")], u64::MAX, (206, (0, u64::MAX))),
        (
            vec![("range", "bytes=18446744073709551614-18446744073709551615")],
            u64::MAX,
            (206, (u64::MAX - 1, 1)),
        ),
        (vec![("if-match", "W/\"tag\"")], 10, (412, (0, 0))),
        (vec![("if-none-match", ", \"tag\",,")], 10, (304, (0, 0))),
        (vec![("if-none-match", "")], 10, (200, (0, 10))),
        (
            vec![("if-match", "\"other\""), ("if-none-match", "*")],
            10,
            (412, (0, 0)),
        ),
        (
            vec![
                ("if-match", "*"),
                ("if-none-match", "W/\"tag\""),
                ("range", "bytes=99-"),
            ],
            10,
            (304, (0, 0)),
        ),
        (
            vec![("if-none-match", "\"other\""), ("if-none-match", "\"tag\"")],
            10,
            (304, (0, 0)),
        ),
        (
            vec![("range", "bytes=2-"), ("if-range", "\"tag\"")],
            10,
            (206, (2, 8)),
        ),
        (
            vec![("range", "bytes=2-"), ("if-range", "W/\"tag\"")],
            10,
            (200, (0, 10)),
        ),
        (
            vec![("range", "bytes=99-"), ("if-range", "date")],
            10,
            (200, (0, 10)),
        ),
        (
            vec![
                ("range", "bytes=2-"),
                ("if-range", "\"tag\""),
                ("if-range", "\"tag\""),
            ],
            10,
            (200, (0, 10)),
        ),
        (
            vec![
                ("if-unmodified-since", "date"),
                ("if-modified-since", "date"),
            ],
            10,
            (200, (0, 10)),
        ),
    ] {
        let (status, selection) =
            select(&Method::GET, &headers(&values), b"\"tag\"", size).unwrap();
        assert_eq!((status.as_u16(), selection), expected, "{values:?}");
    }
    assert_eq!(
        select(
            &Method::HEAD,
            &headers(&[("range", "bytes=99-")]),
            b"\"tag\"",
            10
        )
        .unwrap(),
        (StatusCode::OK, (0, 10))
    );
    for value in [
        "unquoted",
        "w/\"tag\"",
        "*,\"tag\"",
        "\"tag\" garbage",
        "\"unterminated",
        "\"a\tb\"",
    ] {
        assert!(
            matches!(
                select(
                    &Method::GET,
                    &headers(&[("if-none-match", value)]),
                    b"\"tag\"",
                    10
                ),
                Err(RequestHandlerError::RawRequest(StatusCode::BAD_REQUEST))
            ),
            "{value}"
        );
    }
    assert_eq!(
        tag_matches(
            &headers(&[("if-none-match", "\"a,b\\c\"")]),
            header::IF_NONE_MATCH,
            b"\"a,b\\c\"",
            false
        )
        .unwrap(),
        Some(true)
    );
}

async fn fixture(
    body: &[u8],
) -> (
    InitialAgentFilesService,
    EnvironmentId,
    RouterFileIndexEntry,
) {
    let files = InitialAgentFilesService::new(Arc::new(InMemoryBlobStorage::new()));
    let environment = EnvironmentId::new();
    let key = files
        .put_if_not_exists(
            environment,
            body.to_vec()
                .map_item(|item| item.map_err(anyhow::Error::from))
                .map_error(anyhow::Error::from),
        )
        .await
        .unwrap();
    (
        files,
        environment,
        RouterFileIndexEntry {
            path: "/internal/a.txt".into(),
            blob_key: key,
            size: body.len() as u64,
        },
    )
}

async fn body(response: RouteExecutionResult) -> Vec<u8> {
    match response.body {
        ResponseBody::NoBody => vec![],
        ResponseBody::Stream(body) => body.into_vec().await.unwrap(),
        _ => panic!("unexpected body"),
    }
}

#[test]
async fn immutable_headers_bodies_and_storage_failures() {
    let (files, environment, mut entry) = fixture(b"abc").await;
    let response = serve(&files, environment, &entry, &Method::GET, &HeaderMap::new())
        .await
        .unwrap();
    assert_eq!(
        response.headers[header::ETAG],
        "\"blake3-6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85\""
    );
    assert_eq!(response.headers[header::CONTENT_TYPE], "text/plain");
    assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(response.headers[header::ACCEPT_RANGES], "bytes");
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-cache");
    assert!(!response.headers.contains_key(header::LAST_MODIFIED));
    assert_eq!(body(response).await, b"abc");
    for (method, values, status, length, expected) in [
        (
            Method::GET,
            vec![("range", "bytes=1-")],
            206,
            Some("2"),
            b"bc".as_slice(),
        ),
        (
            Method::HEAD,
            vec![("range", "bytes=1-")],
            200,
            Some("3"),
            b"".as_slice(),
        ),
        (
            Method::GET,
            vec![("if-none-match", "*")],
            304,
            None,
            b"".as_slice(),
        ),
        (
            Method::GET,
            vec![("if-match", "\"wrong\"")],
            412,
            Some("0"),
            b"".as_slice(),
        ),
        (
            Method::GET,
            vec![("range", "bytes=3-")],
            416,
            Some("0"),
            b"".as_slice(),
        ),
    ] {
        let response = serve(&files, environment, &entry, &method, &headers(&values))
            .await
            .unwrap();
        assert_eq!(response.status.as_u16(), status);
        assert_eq!(
            response
                .headers
                .get(header::CONTENT_LENGTH)
                .map(|v| v.to_str().unwrap()),
            length
        );
        assert_eq!(body(response).await, expected);
        assert!(matches!(
            serve(
                &files,
                EnvironmentId::new(),
                &entry,
                &method,
                &headers(&values)
            )
            .await,
            Err(RequestHandlerError::InternalError(_))
        ));
        entry.size += 1;
        assert!(matches!(
            serve(&files, environment, &entry, &method, &headers(&values)).await,
            Err(RequestHandlerError::InternalError(_))
        ));
        entry.size -= 1;
    }
    for (path, mime) in [
        ("/a.html", "text/html"),
        ("/a.json", "application/json"),
        ("/a.unknown-extension", "application/octet-stream"),
    ] {
        entry.path = path.into();
        let response = serve(
            &files,
            environment,
            &entry,
            &Method::HEAD,
            &HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.headers[header::CONTENT_TYPE], mime);
    }
    let (files, environment, entry) = fixture(b"").await;
    let response = serve(&files, environment, &entry, &Method::GET, &HeaderMap::new())
        .await
        .unwrap();
    assert_eq!(response.headers[header::CONTENT_LENGTH], "0");
    assert!(body(response).await.is_empty());
}

#[test]
async fn storage_stream_verifies_eof_before_releasing_last_byte() {
    let chunks = || vec![Ok(Bytes::from_static(b"ab")), Ok(Bytes::from_static(b"c"))];
    let success: Vec<Bytes> = verified_stream(futures::stream::iter(chunks()).boxed(), 3)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(success.concat(), b"abc");
    for (mut chunks, size) in [(chunks(), 4), (chunks(), 2), (chunks(), 3)] {
        if size == 3 {
            chunks.push(Err(anyhow::anyhow!("private storage error")));
        }
        let output = verified_stream(futures::stream::iter(chunks).boxed(), size)
            .collect::<Vec<_>>()
            .await;
        assert!(output.last().unwrap().is_err());
        assert!(
            output
                .iter()
                .filter_map(|v| v.as_ref().ok())
                .map(|bytes| bytes.len() as u64)
                .sum::<u64>()
                < size
        );
        assert_eq!(
            output.last().unwrap().as_ref().unwrap_err().to_string(),
            "File storage stream failed"
        );
    }
}

#[test]
#[test_r::timeout("20s")]
async fn late_blob_failure_aborts_fixed_length_http1_and_http2() {
    use tokio_util::task::AbortOnDropHandle;
    for http2 in [false, true] {
        let fail = Arc::new(tokio::sync::Notify::new());
        let trigger = fail.clone();
        let endpoint = poem::endpoint::make(move |_| {
            let fail = fail.clone();
            async move {
                let stream = futures::stream::once(async { Ok(Bytes::from_static(b"abc")) })
                    .chain(futures::stream::once(async move {
                        fail.notified().await;
                        Err(anyhow::anyhow!("late storage error"))
                    }))
                    .boxed();
                poem::Response::builder()
                    .header("content-length", "3")
                    .body(poem::Body::from_bytes_stream(verified_stream(stream, 3)))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5));
        let client = if http2 {
            builder.http2_prior_knowledge()
        } else {
            builder.http1_only()
        }
        .build()
        .unwrap();
        let mut response = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.chunk().await.unwrap().unwrap(), b"ab".as_slice());
        trigger.notify_one();
        assert!(
            response.bytes().await.is_err(),
            "Late failure completed a fixed-length response"
        );
    }
}
