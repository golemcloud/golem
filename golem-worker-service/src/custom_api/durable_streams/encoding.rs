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

//! Translation of a stream slot read into its HTTP wire representation: offset
//! text, metadata headers, page bodies, SSE frames and live cursors.

use super::super::error::RequestHandlerError;
use super::super::{ResponseBody, RouteExecutionResult};
use super::expiry::{add_expiry_headers, cache_control};
use super::response;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::workerexecutor::v1::{ReadStreamSlotSuccess, stream_slot_item};
use golem_common::model::OplogIndex;
use golem_common::model::durable_stream::StreamOffset;
use golem_common::schema::SchemaValue;
use golem_schema::schema::render::to_json_value;
use golem_service_base::custom_api::{DurableStreamRepresentation, DurableStreamSlot};
use http::{HeaderName, HeaderValue, StatusCode};
use prost::Message;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn offset_text(v: &[u8]) -> Result<String, RequestHandlerError> {
    let offset = if v.is_empty() {
        StreamOffset::new(OplogIndex::NONE, 0)
    } else {
        let bytes = v
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid internal stream offset length"))?;
        StreamOffset::from_bytes(bytes).map_err(anyhow::Error::msg)?
    };
    Ok(offset.to_string())
}

pub(super) fn etag(
    r: &ReadStreamSlotSuccess,
    start: &str,
    end: &[u8],
) -> Result<String, RequestHandlerError> {
    Ok(format!(
        "\"{}:{}:{}\"",
        r.stream_identity,
        start,
        offset_text(end)?
    ))
}

/// Response describing the stream without any items. With `head` set, the
/// response is for a HEAD request: it has no body and reports the head offset
/// and current EOF state instead of a paging position.
pub(super) fn metadata_response(
    r: &ReadStreamSlotSuccess,
    head: bool,
    slot: &DurableStreamSlot,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    if r.tombstoned {
        return Ok(response(StatusCode::GONE));
    }
    let mut out = response(StatusCode::OK);
    headers(&mut out, r, head, slot)?;
    out.headers.insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    if head {
        add_expiry_headers(&mut out, &r.expiry_policy);
    }
    out.headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    out.headers.insert(
        http::header::ETAG,
        etag(r, &offset_text(&[])?, &r.head_offset)?
            .parse()
            .map_err(anyhow::Error::from)?,
    );
    if head {
        out.headers.insert(
            HeaderName::from_static("stream-next-offset"),
            offset_text(&r.head_offset)?
                .parse()
                .map_err(anyhow::Error::from)?,
        );
    }
    if !head {
        out.body = ResponseBody::PoemBody {
            body: poem::Body::empty(),
            content_type: None,
        };
    }
    Ok(out)
}

fn headers(
    out: &mut RouteExecutionResult,
    r: &ReadStreamSlotSuccess,
    head: bool,
    slot: &DurableStreamSlot,
) -> Result<(), RequestHandlerError> {
    out.headers.insert(
        http::header::CONTENT_TYPE,
        slot.content_type.parse().map_err(anyhow::Error::from)?,
    );
    out.headers.insert(
        HeaderName::from_static("stream-next-offset"),
        offset_text(&r.next_offset)?
            .parse()
            .map_err(anyhow::Error::from)?,
    );
    out.headers.insert(
        HeaderName::from_static("stream-closed"),
        HeaderValue::from_static(if r.closed && (head || r.up_to_date) {
            "true"
        } else {
            "false"
        }),
    );
    if r.cancelled && (head || r.up_to_date) {
        out.headers.insert(
            HeaderName::from_static("stream-cancelled"),
            HeaderValue::from_static("true"),
        );
    }
    if r.up_to_date {
        out.headers.insert(
            HeaderName::from_static("stream-up-to-date"),
            HeaderValue::from_static("true"),
        );
    }
    Ok(())
}

/// One page of items. A closed historic page (not yet up to date) is immutable
/// and cacheable; every other page must not be cached.
pub(super) fn data_response(
    r: &ReadStreamSlotSuccess,
    sensitive: bool,
    slot: &DurableStreamSlot,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let body = render_items(r, slot.representation)?;
    let mut out = response(StatusCode::OK);
    out.body = ResponseBody::PoemBody {
        body: poem::Body::from_bytes(body.into()),
        content_type: None,
    };
    headers(&mut out, r, false, slot)?;
    out.headers.insert(
        http::header::CACHE_CONTROL,
        cache_control(
            &r.expiry_policy,
            r.expiry_deadline_millis,
            !sensitive && r.closed && !r.up_to_date,
        )
        .parse()
        .map_err(anyhow::Error::from)?,
    );
    Ok(out)
}

fn render_items(
    r: &ReadStreamSlotSuccess,
    representation: DurableStreamRepresentation,
) -> Result<Vec<u8>, RequestHandlerError> {
    if representation == DurableStreamRepresentation::Bytes {
        let mut bytes = Vec::new();
        for item in &r.items {
            let Some(stream_slot_item::Content::PackedU8(value)) = &item.content else {
                return Err(
                    anyhow::anyhow!("binary stream item is not a packed byte payload").into(),
                );
            };
            bytes.extend_from_slice(value);
        }
        return Ok(bytes);
    }
    let graph = r
        .element_schema
        .clone()
        .ok_or_else(|| anyhow::anyhow!("stream schema missing"))?
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid stream schema: {e}"))?;
    let mut values = Vec::new();
    for item in &r.items {
        let Some(stream_slot_item::Content::Value(bytes)) = &item.content else {
            return Err(anyhow::anyhow!("JSON stream item is not a schema value").into());
        };
        let proto = ProtoSchemaValue::decode(bytes.as_slice()).map_err(anyhow::Error::from)?;
        let value: SchemaValue = proto.try_into().map_err(anyhow::Error::msg)?;
        values.push(to_json_value(&graph, &graph.root, &value).map_err(anyhow::Error::msg)?);
    }
    serde_json::to_vec(&values).map_err(|e| anyhow::anyhow!(e).into())
}

/// Encodes one batch as SSE frames: an optional `data` event followed by a
/// `control` event. Binary payloads are base64 encoded. Until the stream is
/// closed and up to date the control event carries a fresh live cursor, which
/// is stored back into `cursor` for the next batch.
pub(super) fn sse_batch(
    r: &ReadStreamSlotSuccess,
    cursor: &mut Option<u64>,
    representation: DurableStreamRepresentation,
) -> Result<Vec<u8>, RequestHandlerError> {
    use base64::Engine;
    let binary = representation == DurableStreamRepresentation::Bytes;
    let mut body = String::new();
    if !r.items.is_empty() {
        let bytes = render_items(r, representation)?;
        let data = if binary {
            base64::engine::general_purpose::STANDARD.encode(bytes)
        } else {
            String::from_utf8(bytes).map_err(anyhow::Error::from)?
        };
        body.push_str("event: data\ndata: ");
        body.push_str(&data);
        body.push_str("\n\n");
    }
    let mut control = serde_json::json!({
        "streamNextOffset": offset_text(&r.next_offset)?,
        "upToDate": r.up_to_date,
    });
    if r.closed && r.up_to_date {
        control["streamClosed"] = true.into();
    } else {
        let next = live_cursor(*cursor)?;
        control["streamCursor"] = next.to_string().into();
        *cursor = Some(next);
    }
    body.push_str(&format!("event: control\ndata: {control}\n\n",));
    Ok(body.into_bytes())
}

pub(super) fn live_cursor(previous: Option<u64>) -> Result<u64, RequestHandlerError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(anyhow::Error::from)?
        .as_secs();
    interval_cursor(seconds, previous, fastrand::u64(1..=180))
}

fn interval_cursor(
    seconds: u64,
    previous: Option<u64>,
    jitter: u64,
) -> Result<u64, RequestHandlerError> {
    // Protocol epoch: October 9, 2024; each cursor interval is 20 seconds.
    let interval = seconds.saturating_sub(1_728_432_000) / 20;
    match previous {
        Some(previous) if previous >= interval => previous
            .checked_add(jitter)
            .ok_or_else(|| anyhow::anyhow!("live cursor overflow").into()),
        _ => Ok(interval),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::workerexecutor::v1::StreamSlotItem;
    use golem_common::schema::{SchemaGraph, SchemaType, schema_value_to_proto_with_streams};
    use std::str::FromStr;
    use test_r::test;

    fn binary_batch() -> ReadStreamSlotSuccess {
        ReadStreamSlotSuccess {
            content_type: "application/octet-stream".into(),
            items: vec![StreamSlotItem {
                offset: Vec::new(),
                content: Some(stream_slot_item::Content::PackedU8(vec![0, 255, 10, 13])),
            }],
            stream_identity: "identity".into(),
            ..Default::default()
        }
    }

    fn slot(representation: DurableStreamRepresentation, content_type: &str) -> DurableStreamSlot {
        DurableStreamSlot {
            canonical_name: "$result".into(),
            public_name: "responses".into(),
            direction: golem_service_base::custom_api::DurableStreamSlotDirection::Output,
            content_type: content_type.into(),
            representation,
        }
    }

    fn binary_slot() -> DurableStreamSlot {
        slot(
            DurableStreamRepresentation::Bytes,
            "application/octet-stream",
        )
    }

    fn control(batch: &ReadStreamSlotSuccess) -> serde_json::Value {
        let text = String::from_utf8(
            sse_batch(batch, &mut None, DurableStreamRepresentation::Bytes).unwrap(),
        )
        .unwrap();
        let control = text.split("event: control\ndata: ").nth(1).unwrap();
        serde_json::from_str(control.trim()).unwrap()
    }

    #[test]
    fn binary_sse_encodes_payload_not_control() {
        let batch = binary_batch();
        let text = String::from_utf8(
            sse_batch(&batch, &mut None, DurableStreamRepresentation::Bytes).unwrap(),
        )
        .unwrap();
        assert!(text.starts_with("event: data\ndata: AP8KDQ==\n\n"));
        let control = control(&batch);
        assert_eq!(control["upToDate"], false);
        assert!(control["streamCursor"].is_string());
        assert!(control.get("streamClosed").is_none());
    }

    #[test]
    fn custom_binary_mime_preserves_raw_bytes_and_uses_base64_sse() {
        let mut batch = binary_batch();
        batch.items[0].content = Some(stream_slot_item::Content::PackedU8(vec![
            0xe2, 0x82, 0xac, 0xe2, 0x82,
        ]));
        let slot = slot(
            DurableStreamRepresentation::Bytes,
            "application/vnd.golem.fragment",
        );

        assert_eq!(
            render_items(&batch, slot.representation).unwrap(),
            vec![0xe2, 0x82, 0xac, 0xe2, 0x82]
        );
        assert_eq!(
            metadata_response(&batch, true, &slot).unwrap().headers[&http::header::CONTENT_TYPE],
            "application/vnd.golem.fragment"
        );
        assert_eq!(
            data_response(&batch, false, &slot).unwrap().headers[&http::header::CONTENT_TYPE],
            "application/vnd.golem.fragment"
        );
        let sse =
            String::from_utf8(sse_batch(&batch, &mut None, slot.representation).unwrap()).unwrap();
        assert!(sse.starts_with("event: data\ndata: 4oKs4oI=\n\n"));
    }

    #[test]
    fn cursor_is_decimal() {
        let cursor = control(&binary_batch())["streamCursor"]
            .as_str()
            .expect("open SSE control event must include a cursor")
            .to_owned();

        assert!(
            !cursor.is_empty() && cursor.bytes().all(|byte| byte.is_ascii_digit()),
            "PROTOCOL.md section 10.1 requires a decimal interval number, got {cursor:?}"
        );
    }

    #[test]
    fn live_cursor_intervals_and_echo_progression() {
        assert_eq!(interval_cursor(1_728_432_019, None, 7).unwrap(), 0);
        assert_eq!(interval_cursor(1_728_432_020, None, 7).unwrap(), 1);
        assert_eq!(interval_cursor(1_728_432_200, Some(9), 7).unwrap(), 10);
        assert_eq!(interval_cursor(1_728_432_200, Some(10), 7).unwrap(), 17);
        assert_eq!(interval_cursor(1_728_432_200, Some(80), 7).unwrap(), 87);
        assert!(interval_cursor(1_728_432_200, Some(u64::MAX), 7).is_err());

        let mut cursor = Some(1_000_000_000);
        for _ in 0..3 {
            let previous = cursor.unwrap();
            let bytes = sse_batch(
                &binary_batch(),
                &mut cursor,
                DurableStreamRepresentation::Bytes,
            )
            .unwrap();
            let text = String::from_utf8(bytes).unwrap();
            let control: serde_json::Value =
                serde_json::from_str(text.split("event: control\ndata: ").nth(1).unwrap().trim())
                    .unwrap();
            let next = control["streamCursor"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert!(next > previous && next <= previous + 180);
            assert_eq!(cursor, Some(next));
        }
    }

    #[test]
    fn rendering_rejects_mixed_payloads_instead_of_dropping_items() {
        let mut batch = binary_batch();
        batch.items.push(StreamSlotItem {
            offset: Vec::new(),
            content: Some(stream_slot_item::Content::Value(Vec::new())),
        });
        assert!(render_items(&batch, DurableStreamRepresentation::Bytes).is_err());
        assert!(sse_batch(&batch, &mut None, DurableStreamRepresentation::Bytes).is_err());
        batch.content_type = "application/json".into();
        batch.element_schema = Some(SchemaGraph::anonymous(SchemaType::string()).into());
        assert!(render_items(&batch, DurableStreamRepresentation::Json).is_err());
        batch.items[0].content = None;
        assert!(render_items(&batch, DurableStreamRepresentation::Json).is_err());
    }

    #[test]
    fn json_sse_preserves_unicode_and_escapes_message_newlines() {
        let values = ["árvíz\nline two", "quotes: \" and \\"];
        let batch = ReadStreamSlotSuccess {
            content_type: "application/json".into(),
            element_schema: Some(SchemaGraph::anonymous(SchemaType::string()).into()),
            items: values
                .iter()
                .map(|value| StreamSlotItem {
                    offset: Vec::new(),
                    content: Some(stream_slot_item::Content::Value(
                        schema_value_to_proto_with_streams(
                            SchemaValue::String((*value).into()),
                            |stream| stream.take_host_endpoint::<u64>(),
                        )
                        .unwrap()
                        .encode_to_vec(),
                    )),
                })
                .collect(),
            ..Default::default()
        };
        let expected = serde_json::json!(["árvíz\nline two", "quotes: \" and \\"]);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &render_items(&batch, DurableStreamRepresentation::Json).unwrap(),
            )
            .unwrap(),
            expected
        );
        let text = String::from_utf8(
            sse_batch(&batch, &mut None, DurableStreamRepresentation::Json).unwrap(),
        )
        .unwrap();
        let data = text
            .strip_prefix("event: data\ndata: ")
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!data.contains('\n'));
        assert!(data.contains("árvíz"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(data).unwrap(),
            expected
        );
    }

    #[test]
    fn closed_historic_page_is_cacheable_but_not_eof() {
        let mut batch = binary_batch();
        batch.closed = true;
        let slot = binary_slot();
        let page = data_response(&batch, false, &slot).unwrap();
        let closed = HeaderName::from_static("stream-closed");
        assert_eq!(page.headers[&closed], HeaderValue::from_static("false"));
        assert_eq!(
            page.headers[&http::header::CACHE_CONTROL],
            HeaderValue::from_static("public, max-age=31536000, immutable")
        );
        assert_eq!(
            data_response(&batch, true, &slot).unwrap().headers[&http::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(
            metadata_response(&batch, true, &slot).unwrap().headers[&closed],
            HeaderValue::from_static("true")
        );
        batch.up_to_date = true;
        let final_page = data_response(&batch, false, &slot).unwrap();
        assert_eq!(final_page.headers[&closed], "true");
        assert_eq!(final_page.headers[&http::header::CACHE_CONTROL], "no-store");
    }

    #[test]
    fn closed_sse_only_signals_eof_after_final_batch() {
        let mut batch = binary_batch();
        batch.closed = true;
        assert!(control(&batch).get("streamClosed").is_none());
        batch.up_to_date = true;
        let final_control = control(&batch);
        assert_eq!(final_control["streamClosed"], true);
        assert!(final_control.get("streamCursor").is_none());
    }

    #[test]
    fn empty_stream_cursor_is_canonical_and_not_a_request_sentinel() {
        let empty = offset_text(&[]).unwrap();
        assert_eq!(empty.len(), 48);
        assert!(StreamOffset::from_str(&empty).is_ok());
        assert_ne!(empty, "-1");
        assert_ne!(empty, "now");
    }

    #[test]
    fn malformed_internal_offsets_fail_instead_of_resetting_the_cursor() {
        let mut reserved = vec![0; 24];
        reserved[0] = 1;
        reserved[20] = 1;
        for invalid in [vec![0; 23], vec![0; 25], vec![0; 24], reserved] {
            assert!(offset_text(&invalid).is_err());
            let mut batch = binary_batch();
            batch.next_offset = invalid.clone();
            let slot = binary_slot();
            assert!(data_response(&batch, false, &slot).is_err());
            assert!(sse_batch(&batch, &mut None, DurableStreamRepresentation::Bytes).is_err());
            assert!(metadata_response(&batch, true, &slot).is_err());
            batch.next_offset.clear();
            batch.head_offset = invalid;
            assert!(metadata_response(&batch, true, &slot).is_err());
        }
    }

    #[test]
    fn head_includes_content_type_and_tail_without_body() {
        let mut batch = binary_batch();
        batch.head_offset = StreamOffset::new(OplogIndex::from_u64(23), 7)
            .as_bytes()
            .to_vec();
        let result = metadata_response(&batch, true, &binary_slot()).unwrap();
        assert!(matches!(result.body, ResponseBody::NoBody));
        assert_eq!(
            result.headers[&http::header::CONTENT_TYPE],
            HeaderValue::from_static("application/octet-stream")
        );
        assert_eq!(
            result.headers[&HeaderName::from_static("stream-next-offset")],
            offset_text(&batch.head_offset)
                .unwrap()
                .parse::<HeaderValue>()
                .unwrap()
        );
        assert_eq!(
            result.headers[&http::header::CACHE_CONTROL],
            HeaderValue::from_static("no-store")
        );
    }

    #[test]
    fn tombstone_metadata_is_not_cacheable() {
        let mut batch = binary_batch();
        batch.tombstoned = true;
        let result = metadata_response(&batch, true, &binary_slot()).unwrap();
        assert_eq!(result.status, StatusCode::GONE);
        assert_eq!(result.headers[&http::header::CACHE_CONTROL], "no-store");
    }
}
