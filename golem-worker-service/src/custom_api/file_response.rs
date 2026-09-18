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
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use std::io;

pub(super) fn representation_headers(
    path: &str,
    size: u64,
    status: StatusCode,
    selection: (u64, u64),
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if status != StatusCode::NOT_MODIFIED {
        headers.insert(header::CONTENT_LENGTH, selection.1.into());
    }
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
        );
    }
    if status.is_success() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime_guess::from_path(path).first_or_octet_stream().as_ref())
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
                    size
                ))
                .unwrap(),
            );
        }
    }
    headers
}

pub(super) fn select(
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
pub(super) fn tag_matches(
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

pub(super) fn verified_stream(
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
