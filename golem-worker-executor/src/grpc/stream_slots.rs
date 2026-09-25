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

//! Transport conversion for the Worker's domain stream-slot operations.

use crate::worker;
use golem_api_grpc::proto::golem::{common, workerexecutor::v1 as proto};
use golem_common::model::durable_stream::{StreamOffset, StreamSessionExpiryPolicy};
use golem_service_base::error::worker_executor::WorkerExecutorError;

impl From<worker::CreateStreamSessionResult> for proto::CreateStreamSessionSuccess {
    fn from(result: worker::CreateStreamSessionResult) -> Self {
        Self {
            session: result.session,
            replayed: result.replayed,
            component_revision: result.component_revision.into(),
            invocation_key: Some(result.invocation_key.into()),
            expiry_policy: Some(expiry_policy_to_proto(result.expiry_policy)),
            expiry_deadline_millis: result.expiry_deadline_millis,
        }
    }
}

fn expiry_policy_to_proto(value: StreamSessionExpiryPolicy) -> proto::StreamSessionExpiryPolicy {
    use proto::stream_session_expiry_policy::Kind;
    proto::StreamSessionExpiryPolicy {
        kind: Some(match value {
            StreamSessionExpiryPolicy::None => Kind::None(common::Empty {}),
            StreamSessionExpiryPolicy::Sliding { ttl_seconds } => Kind::TtlSeconds(ttl_seconds),
            StreamSessionExpiryPolicy::Absolute { expires_at_millis } => {
                Kind::ExpiresAtMillis(expires_at_millis)
            }
        }),
    }
}

pub(crate) fn expiry_policy_from_proto(
    value: Option<proto::StreamSessionExpiryPolicy>,
) -> Result<StreamSessionExpiryPolicy, WorkerExecutorError> {
    use proto::stream_session_expiry_policy::Kind;
    match value.and_then(|value| value.kind) {
        None | Some(Kind::None(_)) => Ok(StreamSessionExpiryPolicy::None),
        Some(Kind::TtlSeconds(ttl_seconds)) => {
            Ok(StreamSessionExpiryPolicy::Sliding { ttl_seconds })
        }
        Some(Kind::ExpiresAtMillis(expires_at_millis)) => {
            Ok(StreamSessionExpiryPolicy::Absolute { expires_at_millis })
        }
    }
}

pub(crate) fn creation_intent_from_proto(
    value: i32,
) -> Result<worker::StreamSessionCreationIntent, WorkerExecutorError> {
    match proto::StreamSessionCreationIntent::try_from(value) {
        Ok(proto::StreamSessionCreationIntent::ExplicitPut) => {
            Ok(worker::StreamSessionCreationIntent::ExplicitPut)
        }
        Ok(proto::StreamSessionCreationIntent::LazyPost) => {
            Ok(worker::StreamSessionCreationIntent::LazyPost)
        }
        Ok(proto::StreamSessionCreationIntent::Unspecified) | Err(_) => Err(
            WorkerExecutorError::invalid_request("stream session creation intent is required"),
        ),
    }
}

impl TryFrom<proto::ReadStreamSlotRequest> for worker::ReadStreamSlotRequest {
    type Error = WorkerExecutorError;

    fn try_from(request: proto::ReadStreamSlotRequest) -> Result<Self, Self::Error> {
        let from_offset = if request.from_offset.is_empty() {
            None
        } else {
            let bytes = request.from_offset.as_slice().try_into().map_err(|_| {
                WorkerExecutorError::invalid_request("offset must contain 24 bytes")
            })?;
            Some(
                StreamOffset::from_bytes(bytes)
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?,
            )
        };
        Ok(Self {
            session: request.session,
            slot: request.slot,
            from_offset,
            max_items: request.max_items,
            max_bytes: request.max_bytes,
            wait_millis: request.wait_millis,
            expected_method: request.expected_method,
            admission: match proto::StreamSlotReadAdmission::try_from(request.admission) {
                Ok(proto::StreamSlotReadAdmission::TouchingOriginGet)
                    if request.invocation_key.is_none() =>
                {
                    worker::StreamSlotReadAdmission::TouchingOriginGet
                }
                Ok(proto::StreamSlotReadAdmission::Head) if request.invocation_key.is_none() => {
                    worker::StreamSlotReadAdmission::Head
                }
                Ok(proto::StreamSlotReadAdmission::Continuation) => {
                    worker::StreamSlotReadAdmission::Continuation(
                        request.invocation_key.map(Into::into).ok_or_else(|| {
                            WorkerExecutorError::invalid_request(
                                "stream read continuation requires an invocation key",
                            )
                        })?,
                    )
                }
                _ => {
                    return Err(WorkerExecutorError::invalid_request(
                        "invalid stream read admission",
                    ));
                }
            },
        })
    }
}

impl From<worker::ReadStreamSlotResult> for proto::ReadStreamSlotSuccess {
    fn from(value: worker::ReadStreamSlotResult) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(|item: worker::StreamSlotItem| proto::StreamSlotItem {
                    offset: item.offset.0.to_vec(),
                    content: Some(match item.content {
                        worker::StreamSlotItemContent::Value(value) => {
                            proto::stream_slot_item::Content::Value(value)
                        }
                        worker::StreamSlotItemContent::PackedU8(bytes) => {
                            proto::stream_slot_item::Content::PackedU8(bytes)
                        }
                    }),
                })
                .collect(),
            next_offset: value
                .next_offset
                .map(|offset| offset.0.to_vec())
                .unwrap_or_default(),
            closed: value.closed,
            cancelled: value.cancelled,
            element_schema: Some(value.element_schema.into()),
            content_type: value.content_type.into(),
            up_to_date: value.up_to_date,
            head_offset: value
                .head_offset
                .map(|offset| offset.0.to_vec())
                .unwrap_or_default(),
            stream_identity: value.stream_identity,
            slots: value.slots,
            tombstoned: value.tombstoned,
            writable: value.writable,
            fork: value.fork,
            invocation_key: Some(value.invocation_key.into()),
            expiry_policy: Some(expiry_policy_to_proto(value.expiry_policy)),
            expiry_deadline_millis: value.expiry_deadline_millis,
        }
    }
}

impl From<proto::AppendToStreamSlotRequest> for worker::AppendToStreamSlotRequest {
    fn from(request: proto::AppendToStreamSlotRequest) -> Self {
        Self {
            session: request.session,
            slot: request.slot,
            payload: request.payload.map(|payload| match payload {
                proto::append_to_stream_slot_request::Payload::Values(values) => {
                    worker::AppendStreamSlotPayload::Values(values.values)
                }
                proto::append_to_stream_slot_request::Payload::PackedU8(bytes) => {
                    worker::AppendStreamSlotPayload::PackedU8(bytes)
                }
            }),
            close: request.close,
            producer: request.producer.map(|producer| worker::StreamSlotProducer {
                id: producer.id,
                epoch: producer.epoch,
                sequence: producer.sequence,
            }),
            expected_method: request.expected_method,
        }
    }
}

impl From<worker::AppendToStreamSlotResult> for proto::AppendToStreamSlotResponse {
    fn from(result: worker::AppendToStreamSlotResult) -> Self {
        use proto::append_to_stream_slot_response::Result as Outcome;
        Self {
            result: Some(match result.outcome {
                worker::AppendStreamSlotOutcome::Accepted(offset) => {
                    Outcome::Accepted(proto::AppendAccepted {
                        offset: offset.0.to_vec(),
                    })
                }
                worker::AppendStreamSlotOutcome::Duplicate {
                    offset,
                    highest_sequence,
                } => Outcome::Duplicate(proto::AppendDuplicate {
                    offset: offset.0.to_vec(),
                    highest_sequence,
                }),
                worker::AppendStreamSlotOutcome::EpochFenced(current_epoch) => {
                    Outcome::EpochFenced(proto::AppendEpochFenced { current_epoch })
                }
                worker::AppendStreamSlotOutcome::SequenceGap { expected, received } => {
                    Outcome::SequenceGap(proto::AppendSequenceGap { expected, received })
                }
                worker::AppendStreamSlotOutcome::Closed => Outcome::Closed(common::Empty {}),
                worker::AppendStreamSlotOutcome::NotFound => Outcome::NotFound(common::Empty {}),
                worker::AppendStreamSlotOutcome::Gone => Outcome::Gone(common::Empty {}),
                worker::AppendStreamSlotOutcome::ReadOnly => Outcome::ReadOnly(common::Empty {}),
            }),
            invocation_key: result.invocation_key.map(Into::into),
            expiry_policy: result.expiry_policy.map(expiry_policy_to_proto),
            expiry_deadline_millis: result.expiry_deadline_millis,
            stream_head_offset: result
                .stream_head_offset
                .map(|offset| offset.0.to_vec())
                .unwrap_or_default(),
            stream_closed: result.stream_closed,
        }
    }
}

impl From<proto::ExportStreamControl> for worker::ExportStreamControlRequest {
    fn from(request: proto::ExportStreamControl) -> Self {
        Self {
            session: request.session,
            slot: request.slot,
            expected_method: request.expected_method,
        }
    }
}

impl From<worker::ExportStreamControlResult> for proto::ExportStreamControlResult {
    fn from(result: worker::ExportStreamControlResult) -> Self {
        match result {
            worker::ExportStreamControlResult::Applied => Self::Applied,
            worker::ExportStreamControlResult::NotFound => Self::NotFound,
            worker::ExportStreamControlResult::Gone => Self::Gone,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::OplogIndex;
    use test_r::test;

    #[test]
    fn stream_slot_read_result_preserves_page_and_item_boundaries() {
        let first = StreamOffset::new(OplogIndex::from_u64(23), 1);
        let second = StreamOffset::new(OplogIndex::from_u64(27), 3);
        let head = StreamOffset::new(OplogIndex::from_u64(31), 0);
        let schema = golem_common::schema::SchemaGraph::empty();
        let expected_schema = schema.clone().into();
        let fork = proto::ForkStreamSlotSuccess {
            replayed: true,
            source_path: "/streams/source/input".into(),
            fork_offset: first.0.to_vec(),
            sub_offset: 2,
            oplog_index: 23,
            invocation_key: Some(golem_common::model::IdempotencyKey::new("fork".into()).into()),
        };
        let result = proto::ReadStreamSlotSuccess::from(worker::ReadStreamSlotResult {
            items: vec![
                worker::StreamSlotItem {
                    offset: first,
                    content: worker::StreamSlotItemContent::Value(vec![7, 2]),
                },
                worker::StreamSlotItem {
                    offset: second,
                    content: worker::StreamSlotItemContent::PackedU8(vec![0, 255, 13]),
                },
            ],
            next_offset: Some(second),
            head_offset: Some(head),
            closed: true,
            cancelled: false,
            element_schema: schema,
            content_type: "application/octet-stream",
            up_to_date: false,
            stream_identity: "identity".into(),
            slots: vec!["a".into(), "b".into()],
            tombstoned: false,
            writable: true,
            fork: Some(fork.clone()),
            invocation_key: golem_common::model::IdempotencyKey::new("invocation".into()),
            expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 30 },
            expiry_deadline_millis: Some(40_000),
        });
        assert_eq!(
            result.items,
            vec![
                proto::StreamSlotItem {
                    offset: first.0.to_vec(),
                    content: Some(proto::stream_slot_item::Content::Value(vec![7, 2]))
                },
                proto::StreamSlotItem {
                    offset: second.0.to_vec(),
                    content: Some(proto::stream_slot_item::Content::PackedU8(vec![0, 255, 13]))
                },
            ]
        );
        assert_eq!(result.next_offset, second.0.to_vec());
        assert_eq!(result.head_offset, head.0.to_vec());
        assert!(result.closed);
        assert!(!result.cancelled);
        assert!(!result.up_to_date);
        assert!(!result.tombstoned);
        assert!(result.writable);
        assert_eq!(result.element_schema, Some(expected_schema));
        assert_eq!(result.content_type, "application/octet-stream");
        assert_eq!(result.stream_identity, "identity");
        assert_eq!(result.slots, vec!["a", "b"]);
        assert_eq!(result.fork, Some(fork));
    }

    #[test]
    fn stream_slot_read_conversion_preserves_offsets_and_limits() {
        let offset = StreamOffset::new(OplogIndex::from_u64(257), 3);
        let request = proto::ReadStreamSlotRequest {
            session: "session-1".into(),
            slot: "input".into(),
            expected_method: "run".into(),
            from_offset: offset.0.to_vec(),
            max_items: 7,
            max_bytes: 203,
            wait_millis: 19,
            admission: proto::StreamSlotReadAdmission::Head as i32,
            ..Default::default()
        };
        let domain = worker::ReadStreamSlotRequest::try_from(request.clone()).unwrap();
        assert_eq!(domain.from_offset, Some(offset));
        assert_eq!(domain.session, "session-1");
        assert_eq!(domain.slot, "input");
        assert_eq!(domain.expected_method, "run");
        assert_eq!(
            (domain.max_items, domain.max_bytes, domain.wait_millis),
            (7, 203, 19)
        );
        for invalid in [vec![0; 23], vec![0; 25], vec![255; 24]] {
            assert!(
                worker::ReadStreamSlotRequest::try_from(proto::ReadStreamSlotRequest {
                    from_offset: invalid,
                    ..request.clone()
                })
                .is_err()
            );
        }
        assert_eq!(
            worker::ReadStreamSlotRequest::try_from(proto::ReadStreamSlotRequest {
                from_offset: vec![],
                ..request
            })
            .unwrap()
            .from_offset,
            None
        );
    }

    #[test]
    fn stream_slot_append_conversion_preserves_absent_and_empty_payloads() {
        use proto::append_to_stream_slot_request::Payload;
        let request = proto::AppendToStreamSlotRequest {
            session: "session-2".into(),
            slot: "bytes".into(),
            expected_method: "write".into(),
            close: true,
            ..Default::default()
        };
        let plain = worker::AppendToStreamSlotRequest::from(request.clone());
        assert!(plain.payload.is_none());
        assert!(plain.producer.is_none());
        assert!(plain.close);
        assert_eq!(plain.session, "session-2");
        assert_eq!(plain.slot, "bytes");
        assert_eq!(plain.expected_method, "write");
        let empty = worker::AppendToStreamSlotRequest::from(proto::AppendToStreamSlotRequest {
            payload: Some(Payload::PackedU8(vec![])),
            ..request.clone()
        });
        assert!(
            matches!(empty.payload, Some(worker::AppendStreamSlotPayload::PackedU8(bytes)) if bytes.is_empty())
        );
        let values = worker::AppendToStreamSlotRequest::from(proto::AppendToStreamSlotRequest {
            payload: Some(Payload::Values(proto::TypedStreamSlotItems {
                values: vec![vec![1, 2], vec![3]],
            })),
            producer: Some(proto::ExternalStreamProducer {
                id: "client-3".into(),
                epoch: 11,
                sequence: 29,
            }),
            ..request
        });
        assert!(
            matches!(values.payload, Some(worker::AppendStreamSlotPayload::Values(values)) if values == vec![vec![1, 2], vec![3]])
        );
        let producer = values.producer.unwrap();
        assert_eq!(producer.id, "client-3");
        assert_eq!(producer.epoch, 11);
        assert_eq!(producer.sequence, 29);
    }

    #[test]
    fn stream_slot_append_outcomes_keep_sequence_coordinates_distinct() {
        use proto::append_to_stream_slot_response::Result as Outcome;
        let result = |outcome| worker::AppendToStreamSlotResult {
            outcome,
            invocation_key: None,
            expiry_policy: None,
            expiry_deadline_millis: None,
            stream_head_offset: None,
            stream_closed: None,
        };
        let offset = StreamOffset::new(OplogIndex::from_u64(531), 2);
        let mut duplicate_result = result(worker::AppendStreamSlotOutcome::Duplicate {
            offset,
            highest_sequence: Some(17),
        });
        duplicate_result.stream_head_offset = Some(offset);
        duplicate_result.stream_closed = Some(true);
        let duplicate = proto::AppendToStreamSlotResponse::from(duplicate_result);
        assert_eq!(
            duplicate.result,
            Some(Outcome::Duplicate(proto::AppendDuplicate {
                offset: offset.0.to_vec(),
                highest_sequence: Some(17)
            }))
        );
        assert_eq!(duplicate.stream_head_offset, offset.0.to_vec());
        assert_eq!(duplicate.stream_closed, Some(true));
        let gap = proto::AppendToStreamSlotResponse::from(result(
            worker::AppendStreamSlotOutcome::SequenceGap {
                expected: 7,
                received: 11,
            },
        ));
        assert_eq!(
            gap.result,
            Some(Outcome::SequenceGap(proto::AppendSequenceGap {
                expected: 7,
                received: 11
            }))
        );
        for (domain, expected) in [
            (
                worker::AppendStreamSlotOutcome::Closed,
                Outcome::Closed(common::Empty {}),
            ),
            (
                worker::AppendStreamSlotOutcome::NotFound,
                Outcome::NotFound(common::Empty {}),
            ),
            (
                worker::AppendStreamSlotOutcome::Gone,
                Outcome::Gone(common::Empty {}),
            ),
            (
                worker::AppendStreamSlotOutcome::ReadOnly,
                Outcome::ReadOnly(common::Empty {}),
            ),
        ] {
            assert_eq!(
                proto::AppendToStreamSlotResponse::from(result(domain)).result,
                Some(expected)
            );
        }
    }
}
