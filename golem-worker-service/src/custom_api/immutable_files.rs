// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::error::RequestHandlerError;
use super::http_completion::{BodyGate, GateTerminal};
use super::http_envelope::{ResponseBodyPolicy, ResponseHead};
use super::{ResponseBody, RouteExecutionResult};
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::RouterFileIndexEntry;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use std::io;

pub(super) async fn serve(
    files: &InitialAgentFilesService,
    environment_id: EnvironmentId,
    entry: &RouterFileIndexEntry,
    method: &Method,
    request_headers: &HeaderMap,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let etag = format!("\"blake3-{}\"", entry.blob_key.0.into_blake3().to_hex());
    let (status, selection) = select(method, request_headers, etag.as_bytes(), entry.size)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if status != StatusCode::NOT_MODIFIED {
        headers.insert(header::CONTENT_LENGTH, selection.1.into());
    }
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{}", entry.size)).unwrap(),
        );
    }
    if status.is_success() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(
                mime_guess::from_path(&entry.path)
                    .first_or_octet_stream()
                    .as_ref(),
            )
            .unwrap(),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        if status == StatusCode::PARTIAL_CONTENT {
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!(
                    "bytes {}-{}/{}",
                    selection.0,
                    selection.0 + selection.1 - 1,
                    entry.size
                ))
                .unwrap(),
            );
        }
    }
    let body = if *method == Method::HEAD || !status.is_success() || selection.1 == 0 {
        // Even conditional responses must not hide a broken deployment index.
        let metadata = files
            .get_metadata(environment_id, entry.blob_key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Indexed initial file blob is missing"))?;
        if metadata.size != entry.size {
            return Err(anyhow::anyhow!(
                "Initial file size differs from deployment index: expected {}, got {}",
                entry.size,
                metadata.size
            )
            .into());
        }
        ResponseBody::NoBody
    } else {
        let opened = files
            .get_range(environment_id, entry.blob_key, selection.0, selection.1)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Indexed initial file blob is missing"))?;
        if opened.total_size != entry.size {
            return Err(anyhow::anyhow!(
                "Initial file size differs from deployment index: expected {}, got {}",
                entry.size,
                opened.total_size
            )
            .into());
        }
        ResponseBody::Stream(poem::Body::from_bytes_stream(verified_stream(
            opened.stream,
            selection.1,
        )))
    };
    Ok(RouteExecutionResult {
        status,
        headers,
        body,
    })
}

fn select(
    method: &Method,
    headers: &HeaderMap,
    etag: &[u8],
    size: u64,
) -> Result<(StatusCode, (u64, u64)), RequestHandlerError> {
    if tag_matches(headers, header::IF_MATCH, etag, true)? == Some(false) {
        return Ok((StatusCode::PRECONDITION_FAILED, (0, 0)));
    }
    if tag_matches(headers, header::IF_NONE_MATCH, etag, false)? == Some(true) {
        return Ok((StatusCode::NOT_MODIFIED, (0, 0)));
    }
    if *method != Method::HEAD
        && let Some(range) = single_header(headers, header::RANGE).and_then(parse_range)
        && (!headers.contains_key(header::IF_RANGE)
            || single_header(headers, header::IF_RANGE).map(trim_ows) == Some(etag))
    {
        return Ok(match range.resolve(size) {
            Some(selection) => (StatusCode::PARTIAL_CONTENT, selection),
            None => (StatusCode::RANGE_NOT_SATISFIABLE, (0, 0)),
        });
    }
    Ok((StatusCode::OK, (0, size)))
}

fn single_header(headers: &HeaderMap, name: header::HeaderName) -> Option<&[u8]> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    values.next().is_none().then_some(first.as_bytes())
}

fn trim_ows(value: &[u8]) -> &[u8] {
    value.trim_ascii_start().trim_ascii_end()
}

/// Entity tags are opaque bytes, not quoted strings with backslash escapes.
fn tag_matches(
    headers: &HeaderMap,
    name: header::HeaderName,
    etag: &[u8],
    strong: bool,
) -> Result<Option<bool>, RequestHandlerError> {
    if !headers.contains_key(&name) {
        return Ok(None);
    }
    let values = headers
        .get_all(name)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect::<Vec<_>>();
    let joined = values.join(b",".as_slice());
    let mut value = trim_ows(&joined);
    if value == b"*" {
        return Ok(Some(true));
    }
    let invalid = || RequestHandlerError::RawRequest(StatusCode::BAD_REQUEST);
    let mut matched = false;
    loop {
        // HTTP list fields allow empty elements, including a trailing comma.
        while let Some(rest) = value.strip_prefix(b",") {
            value = trim_ows(rest);
        }
        if value.is_empty() {
            return Ok(Some(matched));
        }
        let weak = value.starts_with(b"W/");
        if weak {
            value = &value[2..];
        }
        if value.first() != Some(&b'"') {
            return Err(invalid());
        }
        let end = value[1..]
            .iter()
            .position(|byte| *byte == b'"')
            .ok_or_else(invalid)?
            + 1;
        if value[1..end]
            .iter()
            .any(|byte| !matches!(*byte, 0x21 | 0x23..=0x7e | 0x80..=0xff))
        {
            return Err(invalid());
        }
        matched |= (!strong || !weak) && &value[..=end] == etag;
        value = trim_ows(&value[end + 1..]);
        if value.is_empty() {
            return Ok(Some(matched));
        }
        if value.first() != Some(&b',') {
            return Err(invalid());
        }
        value = trim_ows(&value[1..]);
    }
}

#[derive(Debug, PartialEq)]
enum ByteRange {
    From(u64, Option<u64>),
    Suffix(u64),
}

impl ByteRange {
    fn resolve(self, size: u64) -> Option<(u64, u64)> {
        match self {
            Self::From(start, end) if start < size => {
                Some((start, end.unwrap_or(size - 1).min(size - 1) - start + 1))
            }
            Self::Suffix(length) if length > 0 && size > 0 => {
                let length = length.min(size);
                Some((size - length, length))
            }
            _ => None,
        }
    }
}

fn parse_range(value: &[u8]) -> Option<ByteRange> {
    let value = trim_ows(value).strip_prefix(b"bytes=")?;
    let dash = value.iter().position(|byte| *byte == b'-')?;
    let (start, end) = (&value[..dash], &value[dash + 1..]);
    fn number(value: &[u8]) -> Option<u64> {
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(value).ok()?.parse().ok()
    }
    if start.is_empty() {
        Some(ByteRange::Suffix(number(end)?))
    } else {
        let start = number(start)?;
        let end = if end.is_empty() {
            None
        } else {
            Some(number(end)?)
        };
        if end.is_some_and(|end| end < start) {
            return None;
        }
        Some(ByteRange::From(start, end))
    }
}

fn verified_stream(
    stream: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    length: u64,
) -> BoxStream<'static, Result<Bytes, io::Error>> {
    let mut gate = BodyGate::new(&ResponseHead {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        content_length: Some(length),
        body_policy: ResponseBodyPolicy::Stream,
    });
    gate.session_success();
    futures::stream::unfold(Some((stream, gate)), |state| async move {
        let (mut stream, mut gate) = state?;
        loop {
            let output = match stream.next().await {
                Some(Ok(bytes)) => gate.push(bytes),
                Some(Err(_)) => gate.producer_failure(),
                None => gate.body_eof(),
            };
            match output.terminal {
                Some(GateTerminal::Abort(_)) => {
                    return Some((Err(io::Error::other("File storage stream failed")), None));
                }
                Some(GateTerminal::Complete) => return output.bytes.map(|bytes| (Ok(bytes), None)),
                None => {
                    if let Some(bytes) = output.bytes {
                        return Some((Ok(bytes), Some((stream, gate))));
                    }
                }
            }
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests {
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
            let _server = AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(
                acceptor, endpoint,
            )));
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
}
