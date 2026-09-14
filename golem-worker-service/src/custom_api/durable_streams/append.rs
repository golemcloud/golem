// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AppendToStreamSlotRequest, ExternalStreamProducer, TypedStreamSlotItems,
    append_to_stream_slot_request::Payload, append_to_stream_slot_response::Result as Outcome,
};
use golem_common::schema::{FieldSource, SchemaGraph};
use golem_schema::schema::render::from_untrusted_json_value;
use tokio::io::AsyncReadExt;

const MAX_PRODUCER_NUMBER: u64 = (1 << 53) - 1;

impl DurableStreamsHandler {
    pub(super) async fn append(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &golem_common::model::AgentId,
        session: &str,
        slot: &str,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let key = format!("{}:{agent_id}:{session}:{slot}", route.route.environment_id);
        if let Err(rejection) = self.limiter.check_append(&key) {
            return Ok(rejection_response(rejection));
        }
        let metadata = self
            .read_slot(route, agent_id, session, slot, Vec::new(), 0, 0)
            .await?;
        if metadata.as_ref().is_some_and(|m| m.tombstoned) {
            return Ok(response(StatusCode::GONE));
        }
        if metadata.as_ref().is_some_and(|m| !m.writable) {
            return Ok(read_only_response());
        }
        let graph: SchemaGraph = match &metadata {
            Some(metadata) => metadata
                .element_schema
                .clone()
                .ok_or_else(|| anyhow::anyhow!("input stream schema missing"))?
                .try_into()
                .map_err(anyhow::Error::msg)?,
            None => {
                if self
                    .read_slot(route, agent_id, session, "", Vec::new(), 0, 0)
                    .await?
                    .is_some()
                {
                    return Ok(response(StatusCode::NOT_FOUND));
                }
                let Some(field) =
                    behaviour
                        .method_input
                        .input_schema
                        .fields()
                        .iter()
                        .find(|field| {
                            field.name == slot && matches!(field.source, FieldSource::UserSupplied)
                        })
                else {
                    return Ok(if declared_slot_content_type(behaviour, slot)?.is_some() {
                        read_only_response()
                    } else {
                        response(StatusCode::NOT_FOUND)
                    });
                };
                let SchemaType::Stream {
                    inner: Some(inner), ..
                } = behaviour
                    .method_input
                    .graph
                    .resolve_ref(&field.schema)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?
                else {
                    return Ok(response(StatusCode::NOT_FOUND));
                };
                if !behaviour.method_parameters.iter().all(|param| {
                    matches!(
                        param,
                        golem_service_base::custom_api::MethodParameter::Path { .. }
                            | golem_service_base::custom_api::MethodParameter::Query { .. }
                    )
                }) {
                    let path = request
                        .underlying
                        .uri()
                        .path()
                        .rsplit_once("/streams/")
                        .map(|(session_path, _)| session_path)
                        .unwrap_or_default();
                    return Ok(problem(
                        StatusCode::NOT_FOUND,
                        "$",
                        &format!(
                            "Create the session with PUT {path} and its non-stream arguments first"
                        ),
                    ));
                }
                let mut graph = behaviour.method_input.graph.clone();
                graph.root = (**inner).clone();
                graph
            }
        };
        let producer = match producer_headers(request.headers()) {
            Ok(producer) => producer,
            Err(message) => return Ok(problem(StatusCode::BAD_REQUEST, "$headers", &message)),
        };
        let close = request
            .header_string_value("stream-closed")?
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let mut body = Vec::new();
        request
            .underlying
            .take_body()
            .into_async_read()
            .take((self.max_append_body_bytes as u64).saturating_add(1))
            .read_to_end(&mut body)
            .await
            .map_err(anyhow::Error::from)?;
        if body.len() > self.max_append_body_bytes {
            return Ok(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "$",
                "Append body exceeds the configured limit",
            ));
        }
        let binary = matches!(
            graph
                .resolve_ref(&graph.root)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?,
            SchemaType::U8 { .. }
        );
        if !body.is_empty() {
            if !has_header(request, "content-type") {
                return Ok(problem(
                    StatusCode::BAD_REQUEST,
                    "$headers.content-type",
                    "Content-Type is required for an append body",
                ));
            }
            if content_type_mismatch(
                request,
                if binary {
                    "application/octet-stream"
                } else {
                    "application/json"
                },
            )? {
                if let Some(metadata) = metadata.as_ref().filter(|metadata| metadata.closed) {
                    return append_response(
                        Outcome::Closed(Default::default()),
                        producer.as_ref(),
                        metadata,
                    );
                }
                return Ok(response(StatusCode::CONFLICT));
            }
        }
        let payload = match decode_body(&graph, binary, &body, close) {
            Ok(payload) => payload,
            Err((path, detail)) => return Ok(problem(StatusCode::BAD_REQUEST, &path, &detail)),
        };
        if let Some(Payload::Values(items)) = &payload
            && items.values.len() > MAX_ITEMS as usize
        {
            return Ok(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "$",
                &format!("An append may contain at most {MAX_ITEMS} JSON messages"),
            ));
        }
        if metadata.is_none() {
            self.create(request, route, behaviour, agent_id, session)
                .await?;
        }
        let outcome = self
            .worker_service
            .append_to_stream_slot(
                agent_id,
                AppendToStreamSlotRequest {
                    agent_id: Some(agent_id.clone().into()),
                    environment_id: Some(route.route.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    session: session.into(),
                    slot: slot.into(),
                    payload,
                    close,
                    producer: producer.clone(),
                    expected_method: route_method(route).into(),
                },
            )
            .await?;
        // Deletion can race an append or a duplicate. The offset stays tied to the
        // acknowledged batch, but EOF and tombstone metadata describe the current stream.
        let Some(metadata) = self
            .read_slot(route, agent_id, session, slot, Vec::new(), 0, 0)
            .await?
        else {
            return Ok(response(StatusCode::NOT_FOUND));
        };
        if metadata.tombstoned {
            return Ok(response(StatusCode::GONE));
        }
        append_response(outcome, producer.as_ref(), &metadata)
    }
}

fn producer_headers(headers: &http::HeaderMap) -> Result<Option<ExternalStreamProducer>, String> {
    let names = ["producer-id", "producer-epoch", "producer-seq"];
    if names.iter().all(|name| !headers.contains_key(*name)) {
        return Ok(None);
    }
    let mut values = Vec::new();
    for name in names {
        let mut entries = headers.get_all(name).iter();
        let value = entries
            .next()
            .ok_or_else(|| format!("Missing {name}: producer headers are all-or-none"))?;
        if entries.next().is_some() {
            return Err(format!("Repeated {name}"));
        }
        values.push(value.to_str().map_err(|_| format!("Invalid {name}"))?);
    }
    if values[0].is_empty() {
        return Err("Producer-Id must not be empty".into());
    }
    let number = |index: usize| {
        let value = values[index];
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "{} must be a non-negative decimal integer",
                names[index]
            ));
        }
        value
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= MAX_PRODUCER_NUMBER)
            .ok_or_else(|| format!("{} must be at most {MAX_PRODUCER_NUMBER}", names[index]))
    };
    Ok(Some(ExternalStreamProducer {
        id: values[0].into(),
        epoch: number(1)?,
        sequence: number(2)?,
    }))
}

fn decode_body(
    graph: &SchemaGraph,
    binary: bool,
    body: &[u8],
    close: bool,
) -> Result<Option<Payload>, (String, String)> {
    if body.is_empty() {
        return if close {
            Ok(None)
        } else {
            Err((
                "$".into(),
                "Empty append requires Stream-Closed: true".into(),
            ))
        };
    }
    if binary {
        return Ok(Some(Payload::PackedU8(body.to_vec())));
    }
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| ("$".into(), e.to_string()))?;
    let values = match json {
        serde_json::Value::Array(values) => values,
        value => vec![value],
    };
    if values.is_empty() {
        return Err(("$".into(), "An append array must not be empty".into()));
    }
    let values = values
        .iter()
        .enumerate()
        .map(|(index, json)| {
            let value = from_untrusted_json_value(graph, &graph.root, json)
                .map_err(|e| (format!("$[{index}]"), e.to_string()))?;
            let proto: ProtoSchemaValue = value
                .try_into()
                .map_err(|e: String| (format!("$[{index}]"), e))?;
            Ok(proto.encode_to_vec())
        })
        .collect::<Result<Vec<_>, (String, String)>>()?;
    Ok(Some(Payload::Values(TypedStreamSlotItems { values })))
}

fn append_response(
    outcome: Outcome,
    producer: Option<&ExternalStreamProducer>,
    metadata: &ReadStreamSlotSuccess,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let mut result = response(StatusCode::NO_CONTENT);
    let (offset, sequence) = match outcome {
        Outcome::Accepted(value) => {
            crate::metrics::record_durable_stream_append("accepted");
            if producer.is_some() {
                result.status = StatusCode::OK;
            }
            (value.offset, producer.map(|p| p.sequence))
        }
        Outcome::Duplicate(value) => {
            crate::metrics::record_durable_stream_append("duplicate");
            (value.offset, value.highest_sequence)
        }
        Outcome::EpochFenced(value) => {
            crate::metrics::record_durable_stream_append("fenced");
            result.status = StatusCode::FORBIDDEN;
            result.headers.insert(
                HeaderName::from_static("producer-epoch"),
                value.current_epoch.to_string(),
            );
            return Ok(result);
        }
        Outcome::SequenceGap(value) => {
            crate::metrics::record_durable_stream_append("gap");
            result.status = StatusCode::CONFLICT;
            result.headers.insert(
                HeaderName::from_static("producer-expected-seq"),
                value.expected.to_string(),
            );
            result.headers.insert(
                HeaderName::from_static("producer-received-seq"),
                value.received.to_string(),
            );
            return Ok(result);
        }
        Outcome::Closed(_) => {
            result.status = StatusCode::CONFLICT;
            (metadata.head_offset.clone(), None)
        }
        Outcome::Gone(_) => return Ok(response(StatusCode::GONE)),
        Outcome::NotFound(_) => return Ok(response(StatusCode::NOT_FOUND)),
        Outcome::ReadOnly(_) => return Ok(read_only_response()),
        Outcome::Failure(_) => return Err(anyhow::anyhow!("unmapped append failure").into()),
    };
    result.headers.insert(
        HeaderName::from_static("stream-next-offset"),
        offset_text(&offset)?,
    );
    result.headers.insert(
        HeaderName::from_static("stream-closed"),
        metadata.closed.to_string(),
    );
    if let (Some(producer), Some(sequence)) = (producer, sequence) {
        result.headers.insert(
            HeaderName::from_static("producer-epoch"),
            producer.epoch.to_string(),
        );
        result.headers.insert(
            HeaderName::from_static("producer-seq"),
            sequence.to_string(),
        );
    }
    Ok(result)
}

pub(super) fn read_only_response() -> RouteExecutionResult {
    let mut result = response(StatusCode::METHOD_NOT_ALLOWED);
    result
        .headers
        .insert(http::header::ALLOW, "PUT, HEAD, GET, DELETE".into());
    result
}

fn problem(status: StatusCode, path: &str, detail: &str) -> RouteExecutionResult {
    body_response(
        status,
        serde_json::json!({
            "type": "about:blank", "status": status.as_u16(), "path": path, "detail": detail
        })
        .to_string()
        .into_bytes(),
        "application/problem+json",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn producer_headers_validate_each_field_and_both_numeric_boundaries() {
        let mut headers = http::HeaderMap::new();
        assert!(producer_headers(&headers).unwrap().is_none());
        for (name, value) in [("producer-id", "p"), ("producer-epoch", "0")] {
            headers.insert(name, value.parse().unwrap());
            assert!(producer_headers(&headers).is_err());
        }
        headers.insert("producer-seq", "9007199254740991".parse().unwrap());
        let producer = producer_headers(&headers).unwrap().unwrap();
        assert_eq!(producer.epoch, 0);
        assert_eq!(producer.sequence, MAX_PRODUCER_NUMBER);
        for name in ["producer-epoch", "producer-seq"] {
            for invalid in [
                "",
                "-1",
                "1.5",
                "1e2",
                "9007199254740992",
                "18446744073709551616",
            ] {
                let mut bad = headers.clone();
                bad.insert(name, invalid.parse().unwrap());
                assert!(producer_headers(&bad).is_err(), "{name}={invalid}");
            }
            let mut repeated = headers.clone();
            repeated.append(name, "0".parse().unwrap());
            assert!(producer_headers(&repeated).is_err());
        }
        headers.insert("producer-id", "".parse().unwrap());
        assert!(producer_headers(&headers).is_err());
    }

    #[test]
    fn append_framing_is_one_level_and_validates_the_entire_batch() {
        let strings = SchemaGraph::anonymous(SchemaType::string());
        let values = |body: &[u8]| {
            let Some(Payload::Values(items)) = decode_body(&strings, false, body, false).unwrap()
            else {
                panic!("expected typed items");
            };
            items
                .values
                .into_iter()
                .map(|encoded| {
                    let proto = ProtoSchemaValue::decode(encoded.as_slice()).unwrap();
                    let value: SchemaValue = proto.try_into().unwrap();
                    to_json_value(&strings, &strings.root, &value).unwrap()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            values(br#"["a","longer"]"#),
            vec![serde_json::json!("a"), serde_json::json!("longer")]
        );
        assert_eq!(values(br#""single""#), vec![serde_json::json!("single")]);
        for body in [
            br#"["valid",9]"#.as_slice(),
            br#"[["nested"]]"#,
            b"[]",
            b"not json",
        ] {
            assert!(decode_body(&strings, false, body, true).is_err());
        }
        assert_eq!(
            decode_body(&strings, false, br#"["valid",9]"#, false)
                .unwrap_err()
                .0,
            "$[1]"
        );
        assert!(decode_body(&strings, false, b"", false).is_err());
        assert_eq!(decode_body(&strings, false, b"", true).unwrap(), None);
        let bytes = SchemaGraph::anonymous(SchemaType::u8());
        assert_eq!(
            decode_body(&bytes, true, &[0, 255, 10], false).unwrap(),
            Some(Payload::PackedU8(vec![0, 255, 10]))
        );
    }

    #[test]
    fn duplicate_reports_original_offset_and_highest_sequence_not_current_tail() {
        let original = golem_common::model::durable_stream::StreamOffsetV1::new(
            golem_common::model::OplogIndex::from_u64(31),
            2,
        );
        let result = append_response(
            Outcome::Duplicate(
                golem_api_grpc::proto::golem::workerexecutor::v1::AppendDuplicate {
                    offset: original.as_bytes().to_vec(),
                    highest_sequence: Some(9),
                },
            ),
            Some(&ExternalStreamProducer {
                id: "p".into(),
                epoch: 4,
                sequence: 2,
            }),
            &ReadStreamSlotSuccess {
                closed: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.status, StatusCode::NO_CONTENT);
        assert_eq!(
            result.headers[&HeaderName::from_static("stream-next-offset")],
            original.to_string()
        );
        assert_eq!(
            result.headers[&HeaderName::from_static("producer-seq")],
            "9"
        );
        assert_eq!(
            result.headers[&HeaderName::from_static("producer-epoch")],
            "4"
        );
        assert_eq!(
            result.headers[&HeaderName::from_static("stream-closed")],
            "false"
        );
    }
}
