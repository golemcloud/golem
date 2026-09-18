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

use base64::Engine;
use chrono::{DateTime, Utc};
use golem_common::model::oplog::payload::external_durable_stream::*;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Response, StatusCode, Url};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::net::IpAddr;
use std::time::Duration;

const MAX_INTEGER: u64 = (1 << 53) - 1;
const MAX_METADATA: usize = 16 * 1024;
const MAX_SSE_CONTROL_BYTES: usize = 64 * 1024;
const SSE_WIRE_PAYLOAD_FACTOR: usize = 16;
const SSE_RETAINED_PAYLOAD_FACTOR: usize = 4;

// read_body retains up to 2 payloads of capacity, or 3 during growth. Borrowed
// RawValue avoids a DOM but serde_json::Deserializer::ignore_value maintains a
// growable nesting stack: malformed input consisting entirely of '[' can use one
// stack byte per payload byte, or 3 payloads during growth. Reserve 2 + 3 + 1:
// retained body, nesting-stack growth, and exact-capacity append framing. Body
// growth and JSON validation do not overlap. The append allowance is conservative
// even though a response is read only after sending the request body.
const BODY_BUFFER_SLOTS: u64 = 6;
// Each retained SSE buffer is capped at B = 4 * payload + 64 KiB. Allow 2B for
// the line, 3B for data during growth, 2B for pending non-base64 data (into_bytes
// preserves String capacity), and B for base64 decode/validation scratch. These
// lifetimes need not all overlap; their sum deliberately overestimates the peak.
const SSE_BUFFER_SLOTS: u64 = 8;
// The eight SSE slots include eight control-sized allowances. Retain another 24
// for control JSON unescaping/owned fields, URL/query/header copies and allocation
// floors. This is headroom, not an assertion that metadata has 32 live copies.
// Imported request DTOs and reqwest/TLS/socket buffers are outside this codec budget.
const METADATA_RESERVATION_BYTES: u64 = 32 * MAX_SSE_CONTROL_BYTES as u64;

pub(crate) fn read_memory_reservation(
    transport: DurableStreamTransport,
    max_bytes: usize,
) -> Option<u64> {
    let slots = if transport == DurableStreamTransport::Sse {
        SSE_BUFFER_SLOTS * SSE_RETAINED_PAYLOAD_FACTOR as u64
    } else {
        BODY_BUFFER_SLOTS
    };
    (max_bytes as u64)
        .checked_mul(slots)?
        .checked_add(METADATA_RESERVATION_BYTES)
}

pub(crate) fn append_memory_reservation(max_bytes: usize) -> Option<u64> {
    (max_bytes as u64)
        .checked_mul(BODY_BUFFER_SLOTS)?
        .checked_add(METADATA_RESERVATION_BYTES)
}

fn invalid(message: &'static str) -> DurableStreamError {
    DurableStreamError::new(DurableStreamErrorKind::InvalidRequest, message)
}

fn protocol(message: &'static str) -> DurableStreamError {
    DurableStreamError::new(DurableStreamErrorKind::ProtocolError, message)
}

fn too_large() -> DurableStreamError {
    DurableStreamError::new(
        DurableStreamErrorKind::PayloadTooLarge,
        "Batch exceeds the configured limit; retrying with the same limit cannot advance",
    )
}

fn transport(error: reqwest::Error) -> DurableStreamError {
    // reqwest errors can contain the URL, including confidential query parameters.
    if error.is_timeout() {
        DurableStreamError::new(DurableStreamErrorKind::Timeout, "HTTP attempt timed out")
    } else {
        DurableStreamError::new(DurableStreamErrorKind::Transport, "HTTP transport failed")
    }
}

fn valid_header(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_METADATA
        && value.trim() == value
        && HeaderValue::from_str(value).is_ok()
}

fn valid_offset(value: &str, concrete: bool) -> bool {
    valid_header(value)
        && !value
            .chars()
            .any(|c| c.is_whitespace() || ",&=?/".contains(c))
        && (!concrete || (value != "now" && value != "-1"))
}

fn media_type(value: &str) -> Result<mime::Mime, DurableStreamError> {
    if !valid_header(value) {
        return Err(invalid("Invalid content type"));
    }
    let mime: mime::Mime = value.parse().map_err(|_| invalid("Invalid content type"))?;
    if mime.type_() == mime::STAR || mime.subtype() == mime::STAR {
        return Err(invalid("Content type must not contain wildcards"));
    }
    Ok(mime)
}

fn is_json(mime: &mime::Mime) -> bool {
    mime.essence_str() == "application/json"
}

fn validate_url(value: &str, timeout_ms: u64, max_bytes: usize) -> Result<Url, DurableStreamError> {
    if !(1..=300_000).contains(&timeout_ms) || max_bytes == 0 {
        return Err(invalid("Invalid timeout or batch limit"));
    }
    if value.len() > MAX_METADATA || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(invalid("Invalid stream URL"));
    }
    let authority = value
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .filter(|authority| !authority.is_empty())
        .ok_or_else(|| invalid("Invalid stream URL"))?;
    if authority.contains('@') || value.contains('\\') {
        return Err(invalid(
            "Stream URL must not contain userinfo or backslashes",
        ));
    }
    let url = Url::parse(value).map_err(|_| invalid("Invalid stream URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid("Stream URL requires a host"))?;
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Stream URL requires HTTPS (or local HTTP), without userinfo or fragments",
        ));
    }
    // The descriptor must not override the operation's checkpoint or transport.
    if url
        .query_pairs()
        .any(|(key, _)| matches!(key.as_ref(), "offset" | "live" | "cursor"))
    {
        return Err(invalid("Stream URL contains reserved read parameters"));
    }
    Ok(url)
}

/// Validates before authorization and returns the exact canonical URL used by the GET.
pub(crate) fn validate_read(
    request: &DurableStreamReadRequest,
    max_bytes: usize,
) -> Result<Url, DurableStreamError> {
    let mut url = validate_url(&request.url, request.timeout_ms, max_bytes)?;
    if !valid_offset(&request.checkpoint.offset, false)
        || request
            .checkpoint
            .cursor
            .as_ref()
            .is_some_and(|value| !valid_header(value))
    {
        return Err(invalid("Invalid offset or cursor"));
    }
    if request.checkpoint.offset == "now" && request.transport != DurableStreamTransport::CatchUp {
        return Err(invalid("Resolve now with catch-up before live reads"));
    }
    if let Some(content_type) = &request.content_type {
        let mime = media_type(content_type)?;
        if (request.mode == DurableStreamMode::Json) != is_json(&mime) {
            return Err(invalid("Read mode does not match content type"));
        }
    } else if request.transport == DurableStreamTransport::Sse {
        return Err(invalid("SSE requires the original stream content type"));
    }
    let mut query = url.query_pairs_mut();
    if let Some(cursor) = &request.checkpoint.cursor {
        query.append_pair("cursor", cursor);
    }
    match request.transport {
        DurableStreamTransport::CatchUp => {}
        DurableStreamTransport::LongPoll => {
            query.append_pair("live", "long-poll");
        }
        DurableStreamTransport::Sse => {
            query.append_pair("live", "sse");
        }
    }
    query.append_pair("offset", &request.checkpoint.offset);
    drop(query);
    Ok(url)
}

/// Validates metadata and framed size before authorization and memory admission.
/// JSON syntax validation belongs to the admitted service operation because its
/// nesting stack can grow with the payload.
pub(crate) fn preflight_append(
    request: &DurableStreamAppendRequest,
    max_bytes: usize,
) -> Result<Url, DurableStreamError> {
    let url = validate_url(&request.url, request.timeout_ms, max_bytes)?;
    let mime = media_type(&request.content_type)?;
    if !valid_header(&request.producer.id)
        || request.producer.epoch > MAX_INTEGER
        || request.producer.sequence > MAX_INTEGER
    {
        return Err(invalid("Invalid producer identity, epoch or sequence"));
    }
    let length = append_length(&request.payload)?;
    if length > max_bytes {
        return Err(too_large());
    }
    if length == 0 && !request.close {
        return Err(invalid("Empty append requires close"));
    }
    match &request.payload {
        DurableStreamAppendPayload::Json(_) => {
            if !is_json(&mime) {
                return Err(invalid("JSON payload requires application/json"));
            }
        }
        DurableStreamAppendPayload::Bytes(bytes) if is_json(&mime) && !bytes.is_empty() => {
            return Err(invalid(
                "application/json requires individually encoded JSON values",
            ));
        }
        DurableStreamAppendPayload::Bytes(_) => {}
    }
    Ok(url)
}

fn validate_append(
    request: &DurableStreamAppendRequest,
    max_bytes: usize,
) -> Result<Url, DurableStreamError> {
    let url = preflight_append(request, max_bytes)?;
    if let DurableStreamAppendPayload::Json(values) = &request.payload {
        for value in values {
            serde_json::from_str::<&RawValue>(value)
                .map_err(|_| invalid("Invalid JSON append value"))?;
        }
    }
    Ok(url)
}

fn append_length(payload: &DurableStreamAppendPayload) -> Result<usize, DurableStreamError> {
    match payload {
        DurableStreamAppendPayload::Bytes(bytes) => Ok(bytes.len()),
        DurableStreamAppendPayload::Json(values) if values.is_empty() => Ok(0),
        DurableStreamAppendPayload::Json(values) => {
            values.iter().try_fold(1usize, |size, value| {
                size.checked_add(1)
                    .and_then(|size| size.checked_add(value.len()))
                    .ok_or_else(too_large)
            })
        }
    }
}

fn append_body(payload: &DurableStreamAppendPayload) -> Result<Vec<u8>, DurableStreamError> {
    let mut body = Vec::with_capacity(append_length(payload)?);
    match payload {
        DurableStreamAppendPayload::Bytes(bytes) => body.extend_from_slice(bytes),
        DurableStreamAppendPayload::Json(values) if !values.is_empty() => {
            body.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    body.push(b',');
                }
                body.extend_from_slice(value.as_bytes());
            }
            body.push(b']');
        }
        DurableStreamAppendPayload::Json(_) => {}
    }
    Ok(body)
}

fn authorize(
    builder: reqwest::RequestBuilder,
    bearer: Option<&str>,
) -> Result<reqwest::RequestBuilder, DurableStreamError> {
    match bearer {
        None => Ok(builder),
        Some(value) => {
            if !valid_header(value) || !value.bytes().all(|c| c.is_ascii_graphic()) {
                return Err(invalid("Invalid bearer credential"));
            }
            let mut header = HeaderValue::from_str(&format!("Bearer {value}"))
                .map_err(|_| invalid("Invalid bearer credential"))?;
            header.set_sensitive(true);
            Ok(builder.header(reqwest::header::AUTHORIZATION, header))
        }
    }
}

pub(super) async fn read_batch(
    client: &Client,
    request: &DurableStreamReadRequest,
    bearer: Option<&str>,
    max_bytes: usize,
) -> Result<DurableStreamBatch, DurableStreamError> {
    let url = validate_read(request, max_bytes)?;
    let builder = client
        .get(url)
        .timeout(Duration::from_millis(request.timeout_ms))
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    let response = authorize(builder, bearer)?
        .send()
        .await
        .map_err(transport)?;
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::NO_CONTENT {
        return Err(http_error(status, response.headers(), Utc::now()));
    }
    if request.transport == DurableStreamTransport::Sse {
        return read_sse(response, request, max_bytes).await;
    }
    let headers = response.headers();
    let closed = flag(headers, "stream-closed")?;
    let up_to_date = flag(headers, "stream-up-to-date")? || closed;
    let next = checkpoint(
        required_header(headers, "stream-next-offset")?,
        header(headers, "stream-cursor")?,
        closed,
        request.transport == DurableStreamTransport::LongPoll,
    )?;
    let content_type = header(headers, "content-type")?
        .or(if status == StatusCode::NO_CONTENT {
            request.content_type.as_deref()
        } else {
            None
        })
        .ok_or_else(|| protocol("Missing stream content type"))?
        .to_owned();
    check_read_content_type(request, &content_type)?;
    let mut payload = read_body(response, max_bytes).await?;
    if status == StatusCode::NO_CONTENT {
        if !payload.is_empty() || !up_to_date {
            return Err(protocol("Invalid empty live response"));
        }
        payload = empty_payload(request.mode, max_bytes)?;
    }
    validate_read_payload(&payload, request.mode)?;
    if request.checkpoint.offset == "now" && (!empty_batch(&payload, request.mode) || !up_to_date) {
        return Err(protocol("now must return an empty caught-up batch"));
    }
    Ok(DurableStreamBatch {
        payload,
        content_type,
        next,
        up_to_date,
        closed,
    })
}

pub(super) async fn append_batch(
    client: &Client,
    request: &DurableStreamAppendRequest,
    bearer: Option<&str>,
    max_bytes: usize,
) -> Result<DurableStreamAppendReceipt, DurableStreamError> {
    let url = validate_append(request, max_bytes)?;
    let mut builder = client
        .post(url)
        .timeout(Duration::from_millis(request.timeout_ms))
        .header(reqwest::header::CONTENT_TYPE, &request.content_type)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header("producer-id", &request.producer.id)
        .header("producer-epoch", request.producer.epoch)
        .header("producer-seq", request.producer.sequence);
    if request.close {
        builder = builder.header("stream-closed", "true");
    }
    let response = authorize(builder, bearer)?
        .body(append_body(&request.payload)?)
        .send()
        .await
        .map_err(transport)?;
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::NO_CONTENT {
        return Err(http_error(status, response.headers(), Utc::now()));
    }
    let headers = response.headers();
    let epoch = integer_header(headers, "producer-epoch")?
        .ok_or_else(|| protocol("Missing producer epoch"))?;
    let sequence = integer_header(headers, "producer-seq")?
        .ok_or_else(|| protocol("Missing producer sequence"))?;
    if epoch != request.producer.epoch || sequence < request.producer.sequence {
        return Err(protocol("Producer acknowledgement does not match request"));
    }
    if sequence > request.producer.sequence {
        let mut error = DurableStreamError::new(
            DurableStreamErrorKind::ProducerDiverged,
            "Remote producer has advanced beyond this non-pipelined append",
        );
        error.producer_epoch = Some(epoch);
        return Err(error);
    }
    let next_offset = if status == StatusCode::OK {
        Some(required_header(headers, "stream-next-offset")?)
    } else {
        header(headers, "stream-next-offset")?
    }
    .map(|offset| checkpoint(offset, None, true, false).map(|next| next.offset))
    .transpose()?;
    let closed = flag(headers, "stream-closed")?;
    if request.close && !closed {
        return Err(protocol("Close was not acknowledged"));
    }
    // A truncated success body is still an uncertain attempt, even though its headers arrived.
    read_body(response, max_bytes).await?;
    Ok(DurableStreamAppendReceipt {
        next_offset,
        epoch,
        sequence,
        closed,
    })
}

fn header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<Option<&'a str>, DurableStreamError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(protocol("Duplicate protocol header"));
    }
    value
        .map(|value| {
            value
                .to_str()
                .map_err(|_| protocol("Invalid protocol header"))
        })
        .transpose()
        .and_then(|value| {
            if value.is_some_and(|value| !valid_header(value)) {
                Err(protocol("Invalid protocol header"))
            } else {
                Ok(value)
            }
        })
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, DurableStreamError> {
    header(headers, name)?.ok_or_else(|| protocol("Missing required protocol header"))
}

fn flag(headers: &HeaderMap, name: &'static str) -> Result<bool, DurableStreamError> {
    match header(headers, name)? {
        None => Ok(false),
        Some("true") => Ok(true),
        _ => Err(protocol("Invalid protocol flag")),
    }
}

fn integer_header(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<u64>, DurableStreamError> {
    header(headers, name)?
        .map(|value| {
            if !value.bytes().all(|c| c.is_ascii_digit()) {
                return Err(protocol("Invalid producer integer"));
            }
            value
                .parse::<u64>()
                .ok()
                .filter(|value| *value <= MAX_INTEGER)
                .ok_or_else(|| protocol("Invalid producer integer"))
        })
        .transpose()
}

fn checkpoint(
    offset: &str,
    cursor: Option<&str>,
    closed: bool,
    live: bool,
) -> Result<DurableStreamCheckpoint, DurableStreamError> {
    if !valid_offset(offset, true)
        || cursor.is_some_and(|value| !valid_header(value))
        || (live && !closed && cursor.is_none())
    {
        return Err(protocol("Invalid or missing checkpoint"));
    }
    Ok(DurableStreamCheckpoint {
        offset: offset.to_owned(),
        cursor: cursor.map(str::to_owned),
    })
}

fn http_error(status: StatusCode, headers: &HeaderMap, now: DateTime<Utc>) -> DurableStreamError {
    let epoch = match integer_header(headers, "producer-epoch") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let expected = match integer_header(headers, "producer-expected-seq") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = integer_header(headers, "producer-received-seq") {
        return error;
    }
    let closed = match flag(headers, "stream-closed") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let kind = match status.as_u16() {
        401 => DurableStreamErrorKind::PermissionDenied,
        403 if epoch.is_some() => DurableStreamErrorKind::Fenced,
        403 => DurableStreamErrorKind::PermissionDenied,
        404 => DurableStreamErrorKind::NotFound,
        410 => DurableStreamErrorKind::Gone,
        409 if closed => DurableStreamErrorKind::Closed,
        409 if expected.is_some() => DurableStreamErrorKind::SequenceConflict,
        413 => DurableStreamErrorKind::PayloadTooLarge,
        429 => DurableStreamErrorKind::RateLimited,
        500..=599 => DurableStreamErrorKind::Unavailable,
        _ => DurableStreamErrorKind::ProtocolError,
    };
    let mut error = DurableStreamError::new(
        kind,
        if status == StatusCode::CONFLICT && !closed && expected.is_none() {
            "HTTP 409: stream content type conflict".to_owned()
        } else {
            format!("Durable Streams HTTP status {}", status.as_u16())
        },
    );
    error.producer_epoch = epoch;
    error.expected_sequence = expected;
    error.retry_after_ms = header(headers, "retry-after")
        .ok()
        .flatten()
        .and_then(|value| retry_after(value, now));
    error
}

fn retry_after(value: &str, now: DateTime<Utc>) -> Option<u64> {
    if value.bytes().all(|c| c.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000));
    }
    let date = DateTime::parse_from_rfc2822(value).ok()?;
    Some(date.signed_duration_since(now).num_milliseconds().max(0) as u64)
}

async fn read_body(
    mut response: Response,
    max_bytes: usize,
) -> Result<Vec<u8>, DurableStreamError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn check_read_content_type(
    request: &DurableStreamReadRequest,
    value: &str,
) -> Result<(), DurableStreamError> {
    let mime = media_type(value).map_err(|_| protocol("Invalid response content type"))?;
    if (request.mode == DurableStreamMode::Json) != is_json(&mime)
        || request
            .content_type
            .as_ref()
            .is_some_and(|expected| media_type(expected).is_ok_and(|expected| expected != mime))
    {
        return Err(protocol(
            "Response content type changed or does not match read mode",
        ));
    }
    Ok(())
}

fn validate_read_payload(
    payload: &[u8],
    mode: DurableStreamMode,
) -> Result<(), DurableStreamError> {
    if mode == DurableStreamMode::Json {
        let raw: &RawValue =
            serde_json::from_slice(payload).map_err(|_| protocol("Invalid JSON batch"))?;
        if !raw.get().starts_with('[') {
            return Err(protocol("JSON batch must be an array"));
        }
    }
    Ok(())
}

fn empty_batch(payload: &[u8], mode: DurableStreamMode) -> bool {
    if mode == DurableStreamMode::Bytes {
        payload.is_empty()
    } else {
        std::str::from_utf8(payload).is_ok_and(|value| {
            value
                .trim()
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .is_some_and(|value| value.trim().is_empty())
        })
    }
}

fn empty_payload(mode: DurableStreamMode, max_bytes: usize) -> Result<Vec<u8>, DurableStreamError> {
    if mode == DurableStreamMode::Json {
        if max_bytes < 2 {
            return Err(too_large());
        }
        Ok(b"[]".to_vec())
    } else {
        Ok(Vec::new())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Control {
    stream_next_offset: String,
    stream_cursor: Option<String>,
    #[serde(default)]
    up_to_date: bool,
    #[serde(default)]
    stream_closed: bool,
}

struct SseParser {
    line: Vec<u8>,
    event: String,
    data: String,
    has_data: bool,
    pending: Option<Vec<u8>>,
    skip_lf: bool,
    first_line: bool,
    consumed: usize,
    wire_limit: usize,
    buffer_limit: usize,
    max_bytes: usize,
    base64: bool,
    mode: DurableStreamMode,
}

impl SseParser {
    fn new(max_bytes: usize, base64: bool, mode: DurableStreamMode) -> Self {
        Self {
            line: Vec::new(),
            event: String::new(),
            data: String::new(),
            has_data: false,
            pending: None,
            skip_lf: false,
            first_line: true,
            consumed: 0,
            // Bound all framing, comments and control JSON as well as data. This allows
            // even base64 split one character per CRLF data line, plus 64 KiB metadata.
            // Excessive framing fails at the original checkpoint, never as a partial batch.
            wire_limit: max_bytes
                .saturating_mul(SSE_WIRE_PAYLOAD_FACTOR)
                .saturating_add(MAX_SSE_CONTROL_BYTES),
            // See read_memory_reservation for retained-buffer and transient-growth accounting.
            buffer_limit: max_bytes
                .saturating_mul(SSE_RETAINED_PAYLOAD_FACTOR)
                .saturating_add(MAX_SSE_CONTROL_BYTES),
            max_bytes,
            base64,
            mode,
        }
    }

    fn push(&mut self, byte: u8) -> Result<Option<(Vec<u8>, Control)>, DurableStreamError> {
        if self.consumed == self.wire_limit {
            return Err(too_large());
        }
        self.consumed += 1;
        if self.skip_lf {
            self.skip_lf = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        if byte == b'\r' || byte == b'\n' {
            self.skip_lf = byte == b'\r';
            let bytes = std::mem::take(&mut self.line);
            let line = std::str::from_utf8(&bytes).map_err(|_| protocol("Invalid SSE UTF-8"))?;
            let line = if self.first_line {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            self.first_line = false;
            if line.is_empty() {
                return self.dispatch();
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => {
                    if value.len() > MAX_METADATA {
                        return Err(too_large());
                    }
                    self.event = value.to_owned();
                }
                "data" => {
                    let added = value.len().saturating_add(usize::from(self.has_data));
                    if added > self.buffer_limit.saturating_sub(self.data.len()) {
                        return Err(too_large());
                    }
                    if self.has_data {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                    self.has_data = true;
                }
                _ => {}
            }
        } else {
            if self.line.len() == self.buffer_limit {
                return Err(too_large());
            }
            self.line.push(byte);
        }
        Ok(None)
    }

    fn dispatch(&mut self) -> Result<Option<(Vec<u8>, Control)>, DurableStreamError> {
        let event = std::mem::take(&mut self.event);
        let data = std::mem::take(&mut self.data);
        if !std::mem::take(&mut self.has_data) {
            return Ok(None);
        }
        match event.as_str() {
            "data" => {
                if self.pending.is_some() {
                    return Err(protocol("SSE data missing its control event"));
                }
                let payload = if self.base64 {
                    let mut encoded = data;
                    encoded.retain(|c| c != '\n' && c != '\r');
                    if !encoded.len().is_multiple_of(4) {
                        return Err(protocol("Invalid SSE base64"));
                    }
                    let padding = encoded.bytes().rev().take_while(|c| *c == b'=').count();
                    if padding > 2 {
                        return Err(protocol("Invalid SSE base64"));
                    }
                    if (encoded.len() / 4 * 3).saturating_sub(padding) > self.max_bytes {
                        return Err(too_large());
                    }
                    // decoded_len_estimate includes up to two padding bytes; do not use it
                    // as the payload cap, since a valid boundary-sized payload could fail.
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| protocol("Invalid SSE base64"))?
                } else {
                    data.into_bytes()
                };
                if payload.len() > self.max_bytes {
                    return Err(too_large());
                }
                validate_read_payload(&payload, self.mode)?;
                self.pending = Some(payload);
                Ok(None)
            }
            "control" => {
                if data.len() > MAX_SSE_CONTROL_BYTES {
                    return Err(too_large());
                }
                let control = serde_json::from_str::<Control>(&data)
                    .map_err(|_| protocol("Invalid SSE control event"))?;
                let payload = match self.pending.take() {
                    Some(payload) => payload,
                    None => empty_payload(self.mode, self.max_bytes)?,
                };
                Ok(Some((payload, control)))
            }
            _ => Ok(None),
        }
    }
}

async fn read_sse(
    mut response: Response,
    request: &DurableStreamReadRequest,
    max_bytes: usize,
) -> Result<DurableStreamBatch, DurableStreamError> {
    if response.status() != StatusCode::OK {
        return Err(protocol("SSE requires HTTP 200"));
    }
    let mime = media_type(required_header(response.headers(), "content-type")?)
        .map_err(|_| protocol("Invalid SSE content type"))?;
    if mime.essence_str() != "text/event-stream" {
        return Err(protocol("SSE requires text/event-stream"));
    }
    let content_type = request
        .content_type
        .as_deref()
        .ok_or_else(|| invalid("SSE requires original content type"))?;
    let original = media_type(content_type)?;
    let base64 = match header(response.headers(), "stream-sse-data-encoding")? {
        Some("base64") => true,
        None if original.type_() == mime::TEXT || is_json(&original) => false,
        _ => return Err(protocol("Missing or invalid SSE data encoding")),
    };
    let mut parser = SseParser::new(max_bytes, base64, request.mode);
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        for byte in chunk {
            if let Some((payload, control)) = parser.push(byte)? {
                let next = checkpoint(
                    &control.stream_next_offset,
                    control.stream_cursor.as_deref(),
                    control.stream_closed,
                    true,
                )?;
                return Ok(DurableStreamBatch {
                    payload,
                    content_type: content_type.to_owned(),
                    next,
                    up_to_date: control.up_to_date || control.stream_closed,
                    closed: control.stream_closed,
                });
            }
        }
    }
    Err(DurableStreamError::new(
        DurableStreamErrorKind::Transport,
        "SSE ended before a complete checkpoint",
    ))
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
