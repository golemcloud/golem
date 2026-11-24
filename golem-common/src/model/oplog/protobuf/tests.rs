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

use super::cards::{raw_queued_card_event_from_proto, raw_queued_card_event_to_proto};
use super::spans::{
    public_span_attributes_from_proto, public_span_attributes_to_proto,
    public_span_finished_from_proto, public_span_finished_to_proto, public_span_started_from_proto,
    public_span_started_to_proto,
};
use crate::base_model::OplogIndex;
use crate::model::oplog::{
    OplogEntry, PublicAttribute, PublicAttributeValue, PublicOplogEntry, PublicSpanAttributes,
    PublicSpanFinished, PublicSpanKind, PublicSpanLink, PublicSpanOutcome, PublicSpanStarted,
    StringAttributeValue,
};

#[cfg(test)]
mod memory_reconciliation_tests {
    use super::*;
    use crate::model::Timestamp;
    use crate::model::oplog::public_oplog_entry::MemorySizeReconciledParams;
    use golem_api_grpc::proto::golem::worker::{
        OplogEntry as ProtoPublicOplogEntry, RawOplogEntry,
    };
    use test_r::test;

    #[test]
    fn absolute_memory_reconciliation_roundtrips_in_raw_and_public_oplogs() {
        for bytes in [0, 9_007_199_254_741_009, u64::MAX] {
            let timestamp = Timestamp::now_utc().rounded();
            let raw = OplogEntry::MemorySizeReconciled { timestamp, bytes };
            assert!(raw.is_hint());
            let proto: RawOplogEntry = raw.clone().try_into().unwrap();
            assert_eq!(OplogEntry::try_from(proto).unwrap(), raw);
            let public = PublicOplogEntry::MemorySizeReconciled(MemorySizeReconciledParams {
                timestamp,
                bytes,
            });
            let proto: ProtoPublicOplogEntry = public.clone().try_into().unwrap();
            assert_eq!(PublicOplogEntry::try_from(proto).unwrap(), public);
        }
    }
}

#[cfg(test)]
mod tool_attachment_control_tests {
    use test_r::test;

    use super::*;
    use crate::base_model::oplog::attachment::{
        ByteStreamCloseCause, ByteStreamFailure, ToolAttachmentControl, ToolAttachmentId,
        ToolAttachmentSlot, ToolAttachmentTerminal,
    };
    use crate::model::Timestamp;
    use crate::model::oplog::public_oplog_entry::ToolAttachmentControlParams;
    use crate::schema::IntoTypedSchemaValue;

    #[test]
    fn tool_attachment_control_contract_roundtrips() {
        let timestamp = Timestamp::now_utc().rounded();
        let transitions = [
            ToolAttachmentControl::ReaderDiscarded,
            ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::HostResourceExhausted),
            ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::Application(
                ByteStreamCloseCause::Finished,
            )),
            ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::Application(
                ByteStreamCloseCause::ConsumerCancelled,
            )),
            ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::Application(
                ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled),
            )),
            ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::Application(
                ByteStreamCloseCause::Failed(ByteStreamFailure::Failed("failure".to_string())),
            )),
        ];
        for slot in [
            ToolAttachmentSlot::Stdin,
            ToolAttachmentSlot::Stdout,
            ToolAttachmentSlot::Stderr,
            ToolAttachmentSlot::Output,
        ] {
            let attachment = ToolAttachmentId {
                creation_start: OplogIndex::from_u64(3),
                slot,
            };
            for transition in &transitions {
                for parent in [None, Some(OplogIndex::from_u64(2))] {
                    let raw = OplogEntry::ToolAttachmentControl {
                        timestamp,
                        entity_parent_start_index: parent,
                        attachment,
                        transition: transition.clone(),
                    };
                    assert!(raw.is_hint());
                    assert_eq!(raw.entity_parent_start_index(), parent);
                    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
                        raw.clone().try_into().unwrap();
                    let decoded: OplogEntry = proto.try_into().unwrap();
                    assert_eq!(decoded, raw);
                    let bytes = crate::serialization::serialize(&raw).unwrap();
                    let decoded: OplogEntry = crate::serialization::deserialize(&bytes).unwrap();
                    assert_eq!(decoded, raw);
                }
                let public = PublicOplogEntry::ToolAttachmentControl(ToolAttachmentControlParams {
                    timestamp,
                    attachment: attachment.into_typed_schema_value().unwrap(),
                    transition: transition.clone().into_typed_schema_value().unwrap(),
                });
                let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
                    public.clone().try_into().unwrap();
                let decoded: PublicOplogEntry = proto.try_into().unwrap();
                assert_eq!(decoded, public);
                for value in ["toolattachmentcontrol", "tool-attachment-control"] {
                    assert!(public.matches(&crate::model::lucene::Query::Term {
                        value: value.to_string(),
                    }));
                }
                let json = serde_json::to_value(&public).unwrap();
                assert_eq!(json["type"], "ToolAttachmentControl");
                assert!(json.get("payload").is_none());
                assert_eq!(
                    serde_json::from_value::<PublicOplogEntry>(json).unwrap(),
                    public
                );
                let crate::model::oplog::OplogEnrichmentEntry::Other(raw) =
                    public.try_into().unwrap()
                else {
                    panic!("expected non-Create projection");
                };
                assert_eq!(
                    raw,
                    OplogEntry::ToolAttachmentControl {
                        timestamp,
                        entity_parent_start_index: None,
                        attachment,
                        transition: transition.clone(),
                    }
                );
            }
        }
    }

    #[test]
    fn tool_attachment_control_rejects_invalid_metadata() {
        let timestamp = Timestamp::now_utc();
        let attachment = ToolAttachmentId {
            creation_start: OplogIndex::from_u64(3),
            slot: ToolAttachmentSlot::Output,
        }
        .into_typed_schema_value()
        .unwrap();
        let transition = ToolAttachmentControl::ReaderDiscarded
            .into_typed_schema_value()
            .unwrap();
        for (attachment, transition) in [
            (transition.clone(), transition.clone()),
            (attachment.clone(), attachment.clone()),
        ] {
            let public = PublicOplogEntry::ToolAttachmentControl(ToolAttachmentControlParams {
                timestamp,
                attachment: attachment.clone(),
                transition: transition.clone(),
            });
            let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
                public.clone().try_into().unwrap();
            assert!(PublicOplogEntry::try_from(proto).is_err());
            assert!(crate::model::oplog::OplogEnrichmentEntry::try_from(public).is_err());
            let proto = golem_api_grpc::proto::golem::worker::RawOplogEntry {
                timestamp: Some(timestamp.into()),
                entity_parent_start_index: None,
                entry: Some(golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry::ToolAttachmentControl(
                    golem_api_grpc::proto::golem::worker::RawToolAttachmentControlParameters {
                        attachment: Some(attachment.try_into().unwrap()),
                        transition: Some(transition.try_into().unwrap()),
                    },
                )),
            };
            assert!(OplogEntry::try_from(proto).is_err());
        }
    }
}

#[cfg(test)]
mod observational_start_proto_tests {
    use golem_api_grpc::proto::golem::worker::RawOplogEntry;
    use test_r::test;

    use super::*;
    use crate::model::oplog::payload::host_functions::HostFunctionName;
    use crate::model::oplog::{DurableFunctionType, OplogEntry};
    use crate::model::{OplogIndex, Timestamp};

    #[test]
    fn observational_start_raw_protobuf_roundtrips() {
        let original = OplogEntry::Start {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index: Some(OplogIndex::from_u64(8)),
            function_name: HostFunctionName::Custom("wasi:http/outgoing-handler::handle".into()),
            invocation_id: None,
            observational_owner: Some(OplogIndex::from_u64(7)),
            driver: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        };

        let proto: RawOplogEntry = original.clone().try_into().unwrap();
        let roundtrip = OplogEntry::try_from(proto).unwrap();
        assert_eq!(roundtrip, original);
    }

    #[test]
    fn driven_start_raw_protobuf_roundtrips() {
        let original = OplogEntry::Start {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index: Some(OplogIndex::from_u64(12)),
            function_name: HostFunctionName::Custom("wasi:http/types::incoming-body::read".into()),
            invocation_id: None,
            observational_owner: None,
            driver: Some(OplogIndex::from_u64(21)),
            request: None,
            durable_function_type: DurableFunctionType::ReadRemote,
            span_started: None,
            denial: None,
        };

        let proto: RawOplogEntry = original.clone().try_into().unwrap();
        let roundtrip = OplogEntry::try_from(proto).unwrap();
        assert_eq!(roundtrip, original);
    }

    #[test]
    fn denied_start_raw_protobuf_roundtrips() {
        let original = OplogEntry::Start {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index: None,
            function_name: HostFunctionName::KeyvalueEventualSet,
            invocation_id: None,
            observational_owner: None,
            driver: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: Some("access denied".to_string()),
        };

        let proto: RawOplogEntry = original.clone().try_into().unwrap();
        let roundtrip = OplogEntry::try_from(proto).unwrap();
        assert_eq!(roundtrip, original);
    }

    #[test]
    fn span_lifecycle_raw_protobuf_roundtrips() {
        use std::collections::BTreeMap;

        use crate::model::invocation_context::{AttributeValue, SpanId, TraceId};
        use crate::model::oplog::raw_types::{
            AttributeMap, SpanAttributes, SpanFinished, SpanKind, SpanLink, SpanOutcome,
            SpanStarted,
        };

        let span_id = SpanId::generate();
        let linked_span_id = SpanId::generate();
        let started = SpanStarted {
            span_id: span_id.clone(),
            trace_id: TraceId::generate(),
            trace_states: vec!["origin=one".into()],
            parent_span_id: Some(SpanId::generate()),
            links: vec![SpanLink {
                trace_id: TraceId::generate(),
                span_id: linked_span_id,
                trace_states: vec!["link=other-trace".into()],
            }],
            started_at: Timestamp::now_utc(),
            attributes: AttributeMap(BTreeMap::from([(
                "name".into(),
                AttributeValue::String("rpc".into()),
            )])),
            kind: SpanKind::Client,
        };
        let start = OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("test".into()),
            invocation_id: None,
            observational_owner: None,
            driver: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: Some(Box::new(started)),
            denial: None,
        }
        .rounded();
        let end = OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(1),
            response: None,
            forced_commit: false,
            span_finished: Some(SpanFinished {
                span_id: span_id.clone(),
                finished_at: Timestamp::now_utc(),
                outcome: SpanOutcome::Failed,
            }),
            span_attributes: Some(SpanAttributes {
                span_id: span_id.clone(),
                attributes: AttributeMap(BTreeMap::from([(
                    "result".into(),
                    AttributeValue::String("error".into()),
                )])),
            }),
        }
        .rounded();
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(1),
            partial: None,
            span_finished: Some(SpanFinished {
                span_id,
                finished_at: Timestamp::now_utc(),
                outcome: SpanOutcome::Cancelled,
            }),
        }
        .rounded();

        for original in [start, end, cancelled] {
            let proto: RawOplogEntry = original.clone().try_into().unwrap();
            assert_eq!(OplogEntry::try_from(proto).unwrap(), original);
        }
    }

    fn attributes(keys: &[&str]) -> Vec<PublicAttribute> {
        keys.iter()
            .map(|key| PublicAttribute {
                key: key.to_string(),
                value: PublicAttributeValue::String(StringAttributeValue {
                    value: format!("{key}-value"),
                }),
            })
            .collect()
    }

    /// Sends a message over the gRPC wire format, as the executor does to the worker service.
    fn over_the_wire<M: prost::Message + Default>(message: M) -> M {
        M::decode(message.encode_to_vec().as_slice()).unwrap()
    }

    #[test]
    fn span_lifecycle_public_protobuf_roundtrips_with_attributes_sorted_by_key() {
        use crate::model::invocation_context::{SpanId, TraceId};

        let span_id = SpanId::generate();
        let started = PublicSpanStarted {
            span_id: span_id.clone(),
            trace_id: TraceId::generate(),
            trace_states: vec!["origin=one".into()],
            parent_span_id: Some(SpanId::generate()),
            links: vec![PublicSpanLink {
                trace_id: TraceId::generate(),
                span_id: SpanId::generate(),
                trace_states: vec!["link=other-trace".into()],
            }],
            started_at: Timestamp::now_utc().rounded(),
            attributes: attributes(&[
                "name", "target", "agent", "zone", "method", "budget", "kind", "client",
            ]),
            kind: PublicSpanKind::Client,
        };
        let finished = PublicSpanFinished {
            span_id: span_id.clone(),
            finished_at: Timestamp::now_utc().rounded(),
            outcome: PublicSpanOutcome::Denied,
        };
        let updated = PublicSpanAttributes {
            span_id,
            attributes: attributes(&[
                "result", "bytes", "status", "error", "retries", "latency", "cache", "outcome",
            ]),
        };

        let expected_started = PublicSpanStarted {
            attributes: attributes(&[
                "agent", "budget", "client", "kind", "method", "name", "target", "zone",
            ]),
            ..started.clone()
        };
        let expected_updated = PublicSpanAttributes {
            attributes: attributes(&[
                "bytes", "cache", "error", "latency", "outcome", "result", "retries", "status",
            ]),
            ..updated.clone()
        };

        assert_eq!(
            public_span_started_from_proto(over_the_wire(public_span_started_to_proto(started)))
                .unwrap(),
            expected_started
        );
        assert_eq!(
            public_span_finished_from_proto(over_the_wire(public_span_finished_to_proto(
                finished.clone()
            )))
            .unwrap(),
            finished
        );
        assert_eq!(
            public_span_attributes_from_proto(over_the_wire(public_span_attributes_to_proto(
                updated
            )))
            .unwrap(),
            expected_updated
        );
    }

    #[test]
    fn invocation_context_attributes_are_sorted_by_key_in_the_public_oplog_entry() {
        use nonempty_collections::NEVec;

        use super::super::spans::{decode_public_span_data, encode_public_span_data};
        use crate::model::invocation_context::{AttributeValue, InvocationContextSpan};
        use crate::model::oplog::payload::types::encode_span_data;
        use crate::model::oplog::{PublicSpanData, SpanData};

        fn keys(spans: &[Vec<PublicSpanData>]) -> Vec<String> {
            match &spans[0][0] {
                PublicSpanData::LocalSpan(span) => {
                    span.attributes.iter().map(|a| a.key.clone()).collect()
                }
                PublicSpanData::ExternalSpan(_) => panic!("expected a local span"),
            }
        }

        let span = InvocationContextSpan::local()
            .with_attributes(
                [
                    "kiwi", "apple", "plum", "fig", "banana", "mango", "date", "cherry",
                ]
                .into_iter()
                .map(|key| (key.to_string(), AttributeValue::String(key.to_string())))
                .collect(),
            )
            .build();
        let expected = [
            "apple", "banana", "cherry", "date", "fig", "kiwi", "mango", "plum",
        ];

        // The executor converts the stored span to the public oplog entry.
        let public = encode_span_data(&SpanData::from_chain(&NEVec::new(span)));
        assert_eq!(keys(&public), expected);

        // The worker service receives the public oplog entry from the executor over gRPC.
        let public = encode_public_span_data(
            decode_public_span_data(&public, 0)
                .into_iter()
                .map(over_the_wire)
                .collect(),
        )
        .unwrap();
        assert_eq!(keys(&public), expected);
    }
}

#[cfg(test)]
mod successful_update_proto_tests {
    use std::collections::HashSet;

    use golem_api_grpc::proto::golem::worker::RawOplogEntry;
    use test_r::test;

    use crate::model::component::ComponentRevision;
    use crate::model::oplog::protobuf::tests::replay_divergence_tests;
    use crate::model::oplog::{
        FailedSnapshotAssistedUpdateDetails, FilesystemSnapshotName, OplogEntry,
        SnapshotAssistedUpdateDetails, SnapshotFault, UpdateDescription, UpdateFailureClass,
    };
    use crate::model::regions::OplogRegion;
    use crate::model::{OplogIndex, Timestamp};

    #[test]
    fn raw_successful_update_preserves_canonical_linear_memory_total() {
        let original = OplogEntry::SuccessfulUpdate {
            timestamp: Timestamp::now_utc(),
            target_revision: ComponentRevision::new(2).unwrap(),
            new_component_size: 123,
            new_total_linear_memory_size: Some(456),
            new_active_plugins: HashSet::new(),
            snapshot_assisted_details: Some(SnapshotAssistedUpdateDetails {
                pending_update_index: OplogIndex::from_u64(5),
                source_component_revision: ComponentRevision::new(1).unwrap(),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: OplogIndex::from_u64(3),
            }),
            bootstrap: Some(OplogRegion {
                start: OplogIndex::from_u64(11),
                end: OplogIndex::from_u64(17),
            }),
        };

        let proto: RawOplogEntry = original.clone().try_into().unwrap();
        let roundtrip: OplogEntry = proto.try_into().unwrap();

        assert_eq!(roundtrip, original);
    }

    #[test]
    fn raw_failed_update_preserves_assisted_provenance() {
        let original = OplogEntry::FailedUpdate {
            timestamp: Timestamp::now_utc(),
            target_revision: ComponentRevision::new(2).unwrap(),
            details: Some("suffix diverged".to_string()),
            snapshot_assisted_details: Some(FailedSnapshotAssistedUpdateDetails {
                pending_update_index: OplogIndex::from_u64(5),
                source_component_revision: ComponentRevision::new(1).unwrap(),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: OplogIndex::from_u64(3),
            }),
            update_attempt_index: Some(OplogIndex::from_u64(5)),
            snapshot_fault: Some(SnapshotFault::Incompatible),
            failure_class: UpdateFailureClass::Divergence,
            divergence: replay_divergence_tests::divergences()
                .into_iter()
                .next()
                .map(Box::new),
        };

        let proto: RawOplogEntry = original.clone().try_into().unwrap();
        let roundtrip: OplogEntry = proto.try_into().unwrap();

        assert_eq!(roundtrip, original);
    }

    #[test]
    fn raw_failed_update_preserves_every_snapshot_fault_class_and_divergence() {
        let faults = [
            None,
            Some(SnapshotFault::Unavailable),
            Some(SnapshotFault::Incompatible),
        ];
        let classes = [
            UpdateFailureClass::Divergence,
            UpdateFailureClass::TargetTrap,
            UpdateFailureClass::NotReplayable,
            UpdateFailureClass::TargetRejected,
            UpdateFailureClass::Interrupted,
            UpdateFailureClass::Operational,
            UpdateFailureClass::Other,
        ];
        let divergences = std::iter::once(None)
            .chain(replay_divergence_tests::divergences().into_iter().map(Some))
            .collect::<Vec<_>>();
        let mut cases = Vec::new();
        for snapshot_fault in faults {
            for failure_class in classes {
                for divergence in &divergences {
                    cases.push((snapshot_fault, failure_class, divergence.clone()));
                }
            }
        }
        cases
            .into_iter()
            .for_each(|(snapshot_fault, failure_class, divergence)| {
                let original = OplogEntry::FailedUpdate {
                    timestamp: Timestamp::now_utc(),
                    target_revision: ComponentRevision::new(2).unwrap(),
                    details: None,
                    snapshot_assisted_details: None,
                    update_attempt_index: None,
                    snapshot_fault,
                    failure_class,
                    divergence: divergence.map(Box::new),
                };

                let proto: RawOplogEntry = original.clone().try_into().unwrap();
                let roundtrip: OplogEntry = proto.try_into().unwrap();

                assert_eq!(roundtrip, original);
            });
    }

    #[test]
    fn raw_snapshot_assisted_pending_update_preserves_the_filesystem_snapshot_and_bootstrap() {
        [
            (None, None),
            (
                Some(FilesystemSnapshotName::periodic()),
                Some(OplogRegion {
                    start: OplogIndex::from_u64(3),
                    end: OplogIndex::from_u64(7),
                }),
            ),
        ]
        .into_iter()
        .for_each(|(filesystem_snapshot, snapshot_bootstrap)| {
            let original = OplogEntry::PendingUpdate {
                timestamp: Timestamp::now_utc(),
                description: UpdateDescription::SnapshotAssistedAutomatic {
                    target_revision: ComponentRevision::new(2).unwrap(),
                    source_component_revision: ComponentRevision::new(1).unwrap(),
                    source_revision_start_index: OplogIndex::INITIAL,
                    snapshot_index: OplogIndex::from_u64(3),
                    snapshot_revision: ComponentRevision::new(1).unwrap(),
                    filesystem_snapshot,
                    snapshot_bootstrap,
                },
                update_attempt_index: Some(OplogIndex::from_u64(2)),
            };

            let proto: RawOplogEntry = original.clone().try_into().unwrap();
            let roundtrip: OplogEntry = proto.try_into().unwrap();

            assert_eq!(roundtrip, original);
        });
    }
}

#[cfg(test)]
mod public_tool_result_proto_tests {
    use test_r::test;

    use crate::base_model::oplog::{
        ExternalToolResultParameters, PublicAgentInvocationResult, PublicExternalToolResult,
    };
    use crate::base_model::tool::{
        SerializableCustomToolError, SerializableToolError, SerializableToolInvocationResult,
        SerializableToolRpcError,
    };
    use crate::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};

    fn typed(value: &str) -> TypedSchemaValue {
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String(value.to_string()),
        )
    }

    fn assert_roundtrip(result: PublicExternalToolResult) {
        let original =
            PublicAgentInvocationResult::ExternalTool(ExternalToolResultParameters { result });
        let proto: golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult =
            original.clone().try_into().unwrap();
        let decoded = PublicAgentInvocationResult::try_from(proto).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn public_external_tool_result_protobuf_preserves_success_and_every_error_variant() {
        assert_roundtrip(PublicExternalToolResult::Success(
            SerializableToolInvocationResult {
                result: Some(Box::new(typed("output"))),
            },
        ));
        for error in [
            SerializableToolRpcError::ProtocolError("protocol".into()),
            SerializableToolRpcError::Denied("denied".into()),
            SerializableToolRpcError::NotFound("missing".into()),
            SerializableToolRpcError::RemoteInternalError("internal".into()),
            SerializableToolRpcError::Cancelled,
            SerializableToolRpcError::ResourceExhausted("quota".into()),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidToolName("name".into()),
            )),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidCommandPath(vec!["a".into(), "b".into()]),
            )),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidInput("input".into()),
            )),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::ConstraintViolation("constraint".into()),
            )),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidResult("result".into()),
            )),
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                    name: "custom".into(),
                    payload: typed("custom"),
                })),
            )),
        ] {
            assert_roundtrip(PublicExternalToolResult::Failure(error));
        }
    }
}

#[cfg(test)]
mod read_only_violation_roundtrip {
    use proptest::prelude::*;
    use test_r::test;

    use crate::model::oplog::{AgentError, ReadOnlyViolationError};

    fn read_only_violation_strat() -> impl Strategy<Value = ReadOnlyViolationError> {
        (any::<String>(), any::<String>()).prop_map(|(method, host_function)| {
            ReadOnlyViolationError {
                method,
                host_function,
            }
        })
    }

    proptest! {
        #[test]
        fn agent_error_read_only_violation_protobuf_roundtrip(
            err in read_only_violation_strat(),
        ) {
            let original = AgentError::ReadOnlyViolation(err);
            let proto: golem_api_grpc::proto::golem::worker::AgentError = original.clone().into();
            let roundtrip: AgentError = proto.try_into().unwrap();
            prop_assert_eq!(roundtrip, original);
        }
    }
}

#[cfg(test)]
mod permission_denied_roundtrip {
    use proptest::prelude::*;
    use test_r::test;

    use crate::model::oplog::AgentError;

    proptest! {
        #[test]
        fn agent_error_permission_denied_protobuf_roundtrip(details in any::<String>()) {
            let original = AgentError::PermissionDenied(details);
            let proto: golem_api_grpc::proto::golem::worker::AgentError = original.clone().into();
            let roundtrip: AgentError = proto.try_into().unwrap();
            prop_assert_eq!(roundtrip, original);
        }
    }
}

#[cfg(test)]
mod queued_card_event_proto_tests {
    use golem_api_grpc::proto::golem::worker::card_holder::Holder;
    use golem_api_grpc::proto::golem::worker::raw_queued_card_event::Event;
    use golem_api_grpc::proto::golem::worker::{
        CardHolder, RawQueuedCardEvent, raw_queued_card_event,
    };
    use proptest::prelude::*;
    use test_r::test;
    use uuid::Uuid;

    use crate::base_model::oplog::{
        QueuedCardEvent, QueuedCardEventCard, QueuedCardEventTransferReceived,
    };
    use crate::model::card::{Card, CardId, StoredCard};

    fn stored_card(card_id: CardId) -> StoredCard {
        StoredCard::Concrete(Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })
    }

    #[test]
    fn raw_queued_card_install_without_payload_is_rejected() {
        let card_id = CardId::new();
        let proto = RawQueuedCardEvent {
            event: Some(Event::Install(raw_queued_card_event::Install {
                card_id: Some(card_id.0.into()),
                card: Vec::new(),
            })),
        };

        assert!(
            super::raw_queued_card_event_from_proto(proto).is_err(),
            "raw queued card installs without a persisted card payload poison durable draining"
        );
    }

    #[test]
    fn raw_queued_card_install_without_payload_is_rejected_on_encode() {
        let card_id = CardId::new();
        let event = QueuedCardEvent::Install(QueuedCardEventCard {
            card_id,
            card: None,
        });

        assert!(
            super::raw_queued_card_event_to_proto(event).is_err(),
            "raw queued card installs without a persisted card payload must not encode to an undecodable protobuf"
        );
    }

    #[test]
    fn raw_queued_card_install_with_mismatched_payload_is_rejected_on_encode() {
        let outer_card_id = CardId::new();
        let payload_card_id = CardId::new();
        let event = QueuedCardEvent::Install(QueuedCardEventCard {
            card_id: outer_card_id,
            card: Some(stored_card(payload_card_id)),
        });

        assert!(
            super::raw_queued_card_event_to_proto(event).is_err(),
            "raw queued card installs with mismatched outer card_id and payload must not encode to an undecodable protobuf"
        );
    }

    #[test]
    fn raw_received_card_transfer_protobuf_roundtrip_preserves_receipt_payload() {
        let card_id = CardId::new();
        let source_card_id = CardId::new();
        let event = QueuedCardEvent::TransferReceived(QueuedCardEventTransferReceived {
            transfer_id: Uuid::new_v4(),
            source_card_id,
            card_id,
            card: Some(stored_card(card_id)),
        });

        let encoded = super::raw_queued_card_event_to_proto(event.clone()).unwrap();
        let decoded = super::raw_queued_card_event_from_proto(encoded).unwrap();

        assert_eq!(decoded, event);
    }

    #[test]
    fn raw_received_card_transfer_protobuf_rejects_trailing_payload_bytes() {
        let card_id = CardId::new();
        let event = QueuedCardEvent::TransferReceived(QueuedCardEventTransferReceived {
            transfer_id: Uuid::new_v4(),
            source_card_id: CardId::new(),
            card_id,
            card: Some(stored_card(card_id)),
        });
        let mut encoded = super::raw_queued_card_event_to_proto(event).unwrap();
        let Event::TransferReceived(receipt) = encoded.event.as_mut().unwrap() else {
            unreachable!()
        };
        receipt.card.push(0xff);

        assert!(
            super::raw_queued_card_event_from_proto(encoded).is_err(),
            "a StoredCard payload with trailing bytes is malformed and must be rejected"
        );
    }

    #[test]
    fn raw_queued_card_transfer_with_malformed_payload_is_rejected() {
        let proto = RawQueuedCardEvent {
            event: Some(Event::TransferStarted(
                raw_queued_card_event::TransferStarted {
                    transfer_id: Some(Uuid::new_v4().into()),
                    card_id: Some(CardId::new().0.into()),
                    card: vec![u8::MAX],
                    target_holder: Some(CardHolder {
                        holder: Some(Holder::Account(Uuid::new_v4().into())),
                    }),
                },
            )),
        };

        assert!(
            super::raw_queued_card_event_from_proto(proto).is_err(),
            "malformed raw queued card transfer payloads must be rejected without panicking"
        );
    }

    #[test]
    fn raw_queued_card_install_with_negative_collection_length_is_rejected_without_panicking() {
        let mut card = vec![crate::serialization::SERIALIZATION_VERSION_V3, 0, 0, 0];
        card.extend_from_slice(&[0; 16]);
        card.push(0x03);

        let proto = RawQueuedCardEvent {
            event: Some(Event::Install(raw_queued_card_event::Install {
                card_id: Some(Uuid::nil().into()),
                card,
            })),
        };

        let result = std::panic::catch_unwind(|| super::raw_queued_card_event_from_proto(proto));

        assert!(matches!(result, Ok(Err(_))));
    }

    proptest! {
        #[test]
        fn malformed_v3_raw_queued_card_transfer_never_panics(mut payload in prop::collection::vec(any::<u8>(), 0..128)) {
            payload.insert(0, crate::serialization::SERIALIZATION_VERSION_V3);
            let proto = RawQueuedCardEvent {
                event: Some(Event::TransferStarted(
                    raw_queued_card_event::TransferStarted {
                        transfer_id: Some(Uuid::new_v4().into()),
                        card_id: Some(CardId::new().0.into()),
                        card: payload,
                        target_holder: Some(CardHolder {
                            holder: Some(Holder::Account(Uuid::new_v4().into())),
                        }),
                    },
                )),
            };

            let result = std::panic::catch_unwind(|| super::raw_queued_card_event_from_proto(proto));
            prop_assert!(result.is_ok(), "malformed raw queued card transfer payload panicked");
        }
    }
}

#[cfg(test)]
pub(crate) mod replay_divergence_tests {
    use test_r::test;

    use crate::base_model::OplogIndex;
    use crate::base_model::oplog::{
        DurableInvocationId, PublicDurableFunctionType, ReplayDivergence, ReplayHostCall,
        ReplayStart, ReplayStep, StartIdentityField, UpdateFailureClass,
    };
    use crate::schema::{
        IntoTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
    };
    use golem_api_grpc::proto::golem::worker as proto;
    use golem_schema::Empty;
    use golem_schema::schema::SchemaValueStream;

    fn host_call(function_name: &str, request: Option<&str>) -> ReplayHostCall {
        ReplayHostCall {
            function_name: function_name.to_string(),
            durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
            parent_start_index: Some(OplogIndex::from_u64(4)),
            invocation_id: Some(DurableInvocationId::new(OplogIndex::from_u64(2), 3)),
            observational_owner: Some(OplogIndex::from_u64(5)),
            driver: None,
            carries_request: true,
            request: request
                .map(|request| Box::new(request.to_string().into_typed_schema_value().unwrap())),
        }
    }

    pub(crate) fn divergences() -> Vec<ReplayDivergence> {
        vec![
            ReplayDivergence {
                oplog_index: OplogIndex::from_u64(7),
                expected: ReplayStep::HostCall(host_call("http::send", Some("recorded"))),
                actual: ReplayStep::HostCall(ReplayHostCall {
                    durable_function_type: PublicDurableFunctionType::ReadLocal(Empty {}),
                    parent_start_index: None,
                    invocation_id: None,
                    observational_owner: None,
                    driver: None,
                    carries_request: false,
                    ..host_call("rpc::invoke", Some("replayed"))
                }),
                mismatched: vec![
                    StartIdentityField::FunctionName,
                    StartIdentityField::FunctionType,
                    StartIdentityField::InvocationId,
                    StartIdentityField::ObservationalOwner,
                    StartIdentityField::RequestPresence,
                    StartIdentityField::Parent,
                ],
                replay_start: Some(ReplayStart::full_replay(vec![
                    OplogIndex::from_u64(40),
                    OplogIndex::from_u64(47),
                ])),
            },
            ReplayDivergence {
                oplog_index: OplogIndex::from_u64(3),
                expected: ReplayStep::HostCall(host_call("clocks::now", None)),
                actual: ReplayStep::invocation_end(),
                mismatched: vec![],
                replay_start: Some(ReplayStart::full_replay(vec![])),
            },
            ReplayDivergence {
                oplog_index: OplogIndex::from_u64(9),
                expected: ReplayStep::end_of_history(),
                actual: ReplayStep::HostCall(host_call("random::bytes", Some("replayed"))),
                mismatched: vec![],
                replay_start: Some(ReplayStart::snapshot(
                    OplogIndex::from_u64(6),
                    vec![OplogIndex::from_u64(8), OplogIndex::from_u64(11)],
                )),
            },
            ReplayDivergence::described(
                OplogIndex::from_u64(12),
                "a recorded completion delivery",
                "a discarded completion",
            ),
        ]
    }

    #[test]
    fn replay_divergence_roundtrips_through_protobuf_desert_and_json() {
        for original in divergences() {
            let proto: proto::ReplayDivergence = original.clone().try_into().unwrap();
            let from_proto: ReplayDivergence = proto.try_into().unwrap();
            assert_eq!(from_proto, original);

            let bytes = desert_rust::serialize_to_byte_vec(&original).unwrap();
            let from_desert: ReplayDivergence = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(from_desert, original);

            let json = serde_json::to_value(&original).unwrap();
            let from_json: ReplayDivergence = serde_json::from_value(json).unwrap();
            assert_eq!(from_json, original);
        }
    }

    #[test]
    fn the_text_of_a_divergence_names_where_its_replay_started() {
        const HINT: &str = "a snapshot load that differs from the live state is a common cause";
        let at = |replay_start| ReplayDivergence {
            replay_start,
            ..ReplayDivergence::described(OplogIndex::from_u64(36), "recorded", "replayed")
        };
        let plain = "Unexpected oplog entry at 36: expected recorded, got replayed";
        let index = OplogIndex::from_u64;
        for (replay_start, text) in [
            (None, plain.to_string()),
            (Some(ReplayStart::full_replay(vec![])), plain.to_string()),
            (
                Some(ReplayStart::snapshot(index(30), vec![])),
                format!("{plain}; replay started from the snapshot at oplog index 30; {HINT}"),
            ),
            (
                Some(ReplayStart::snapshot(index(30), vec![index(40), index(47)])),
                format!(
                    "{plain}; replay started from the snapshot at oplog index 30 after the snapshots at oplog indexes 40, 47 were rejected; {HINT}"
                ),
            ),
            (
                Some(ReplayStart::full_replay(vec![index(40)])),
                format!(
                    "{plain}; replay started without a snapshot after the snapshot at oplog index 40 was rejected; {HINT}"
                ),
            ),
            (
                Some(ReplayStart::full_replay(vec![index(40), index(47)])),
                format!(
                    "{plain}; replay started without a snapshot after the snapshots at oplog indexes 40, 47 were rejected; {HINT}"
                ),
            ),
        ] {
            assert_eq!(at(replay_start).to_string(), text);
        }
    }

    fn with_request(function_name: &str, request: TypedSchemaValue) -> ReplayHostCall {
        ReplayHostCall {
            request: Some(Box::new(request)),
            ..host_call(function_name, None)
        }
    }

    #[test]
    fn a_request_without_a_protobuf_form_leaves_only_its_own_host_call() {
        let live_stream = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::stream(None)),
            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(42_u32)),
        );
        let encodable = "recorded".to_string().into_typed_schema_value().unwrap();
        for (expected, actual, expected_kept, actual_kept) in [
            (
                Some(encodable.clone()),
                Some(live_stream.clone()),
                true,
                false,
            ),
            (Some(live_stream), Some(encodable), false, true),
        ] {
            let divergence = ReplayDivergence {
                oplog_index: OplogIndex::from_u64(7),
                expected: ReplayStep::HostCall(ReplayHostCall {
                    request: expected.map(Box::new),
                    ..host_call("http::send", None)
                }),
                actual: ReplayStep::HostCall(ReplayHostCall {
                    request: actual.map(Box::new),
                    ..host_call("rpc::invoke", None)
                }),
                mismatched: vec![StartIdentityField::FunctionName],
                replay_start: Some(ReplayStart::snapshot(OplogIndex::from_u64(5), vec![])),
            };

            let decoded: ReplayDivergence = divergence
                .into_proto_keeping_structure()
                .try_into()
                .unwrap();

            let request_of = |step: &ReplayStep| match step {
                ReplayStep::HostCall(call) => call.request.clone(),
                other => panic!("expected a host call, got {other}"),
            };
            let expected_request = request_of(&decoded.expected);
            let actual_request = request_of(&decoded.actual);
            assert_eq!(expected_request.is_some(), expected_kept);
            assert_eq!(actual_request.is_some(), actual_kept);
            let kept = expected_request.or(actual_request).unwrap();
            assert_eq!(kept.value(), &SchemaValue::String("recorded".to_string()));
            assert_eq!(decoded.mismatched, vec![StartIdentityField::FunctionName]);
        }
    }

    #[test]
    fn a_host_call_with_a_nested_nan_request_equals_its_clone() {
        let nan_request = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::list(SchemaType::option(SchemaType::f64()))),
            SchemaValue::List {
                elements: vec![SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::F64(f64::NAN))),
                }],
            },
        );
        let call = with_request("http::send", nan_request);
        assert_eq!(call, call.clone());

        let divergence = ReplayDivergence {
            oplog_index: OplogIndex::from_u64(7),
            expected: ReplayStep::HostCall(call.clone()),
            actual: ReplayStep::HostCall(call),
            mismatched: vec![],
            replay_start: None,
        };
        assert_eq!(divergence, divergence.clone());

        let other_payload = with_request(
            "http::send",
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::f64()),
                SchemaValue::F64(-0.0),
            ),
        );
        assert_ne!(
            other_payload,
            with_request(
                "http::send",
                TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::f64()),
                    SchemaValue::F64(0.0),
                ),
            )
        );
    }

    #[test]
    fn host_calls_with_different_requests_are_different_calls() {
        assert_ne!(
            host_call("http::send", Some("recorded")),
            host_call("http::send", Some("replayed"))
        );
        assert_eq!(
            host_call("http::send", Some("recorded"))
                .mismatched_fields(&host_call("http::send", Some("replayed"))),
            vec![]
        );
    }

    #[test]
    fn update_failure_class_roundtrips_every_class() {
        for class in [
            UpdateFailureClass::Divergence,
            UpdateFailureClass::TargetTrap,
            UpdateFailureClass::NotReplayable,
            UpdateFailureClass::TargetRejected,
            UpdateFailureClass::Interrupted,
            UpdateFailureClass::Operational,
            UpdateFailureClass::Other,
        ] {
            assert_eq!(
                UpdateFailureClass::from_proto_number(class.proto_number()).unwrap(),
                class
            );
            let bytes = desert_rust::serialize_to_byte_vec(&class).unwrap();
            assert_eq!(
                desert_rust::deserialize::<UpdateFailureClass>(&bytes).unwrap(),
                class
            );
            let json = serde_json::to_string(&class).unwrap();
            assert_eq!(
                serde_json::from_str::<UpdateFailureClass>(&json).unwrap(),
                class
            );
        }
        assert_eq!(
            serde_json::to_string(&UpdateFailureClass::NotReplayable).unwrap(),
            "\"not-replayable\""
        );
    }
}
