// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use crate::base_model::durable_stream::{DurableStreamHandle, StreamMapSide, StreamValuePathStep};
pub use crate::model::invocation_session_public::JsonMethodInput;
use crate::schema::streaming::{
    decode_recursive_stream_value, preflight_proto_recursive_stream_value,
};
use crate::schema::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, SchemaValue, TypedSchemaValue,
    stream::SchemaValueStream,
};
use desert_rust::BinaryCodec;
use golem_schema::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_schema::schema::host_managed::redact_host_managed_value;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};

/// The original method input, before interpreting it against an execution schema.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentMethodInput {
    Native(NativeMethodInput),
    PublicJson(JsonMethodInput),
}

impl TryFrom<SchemaValue> for AgentMethodInput {
    type Error = String;

    fn try_from(value: SchemaValue) -> Result<Self, Self::Error> {
        let value = value.try_into()?;
        let (input, _) = NativeMethodInput::capture(value, Vec::new())?;
        Ok(Self::Native(input))
    }
}

/// Component-independent public oplog representation of the original request.
/// Native stream leaves are replaced by request-local ordinals at the recorded paths.
/// Capability leaves are redacted before projection.
#[derive(IntoSchema, FromSchema)]
#[schema(named = "original-method-input")]
pub enum PublicMethodInput {
    Native {
        value: SchemaValue,
        stream_paths: Vec<Vec<StreamValuePathStep>>,
        streams: Vec<Option<DurableStreamHandle>>,
    },
    PublicJson(String),
}

impl AgentMethodInput {
    /// Projects captured input without resolving a method or an execution revision.
    /// Raw JSON values include fields that execution can subsequently reject.
    pub fn to_public_value(&self) -> Result<TypedSchemaValue, String> {
        let public = match self {
            Self::Native(input) => PublicMethodInput::Native {
                value: redact_host_managed_value(input.value.clone()),
                stream_paths: input.stream_paths.clone(),
                streams: input.streams.clone(),
            },
            Self::PublicJson(input) => PublicMethodInput::PublicJson(input.as_str().to_owned()),
        };
        public
            .into_typed_schema_value()
            .map_err(|error| error.to_string())
    }
}

/// Schema-native data with request-local stream ordinals instead of transport endpoints.
#[derive(Clone, BinaryCodec)]
#[desert(evolution())]
pub struct NativeMethodInput {
    value: SchemaValue,
    stream_paths: Vec<Vec<StreamValuePathStep>>,
    streams: Vec<Option<DurableStreamHandle>>,
    external_rest: bool,
}

impl PartialEq for NativeMethodInput {
    fn eq(&self, other: &Self) -> bool {
        self.same_unbound_value(other) && self.streams == other.streams
    }
}

impl Eq for NativeMethodInput {}

impl Debug for NativeMethodInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMethodInput")
            .field("streams", &self.streams.len())
            .finish_non_exhaustive()
    }
}

impl NativeMethodInput {
    /// Returns the persisted input and the transport ID of each stream ordinal.
    /// Transport IDs belong to the attachment and are not part of request equality.
    pub fn capture(
        input: ProtoSchemaValue,
        foreign_streams: Vec<(u64, DurableStreamHandle)>,
    ) -> Result<(Self, Vec<u64>), String> {
        let transport_ids = preflight_proto_recursive_stream_value(&input)?;
        if transport_ids.iter().copied().collect::<HashSet<_>>().len() != transport_ids.len() {
            return Err("method input references a transport stream more than once".into());
        }
        let has_foreign_streams = !foreign_streams.is_empty();
        let foreign_count = foreign_streams.len();
        if has_foreign_streams && foreign_count != transport_ids.len() {
            return Err("method input foreign mappings do not match its stream references".into());
        }
        let mut foreign_streams: HashMap<_, _> = foreign_streams.into_iter().collect();
        if foreign_streams.len() != foreign_count {
            return Err("method input contains duplicate foreign stream mappings".into());
        }
        let mut streams = Vec::with_capacity(transport_ids.len());
        let mut stream_paths = Vec::with_capacity(transport_ids.len());
        let mut value = decode_recursive_stream_value(input, |transport_id, path| {
            let handle = foreign_streams.remove(&transport_id);
            if has_foreign_streams && handle.is_none() {
                return Err("method input references an unmapped foreign stream".into());
            }
            let ordinal = streams.len() as u64;
            streams.push(handle);
            stream_paths.push(path.to_vec());
            Ok(SchemaValueStream::from_host_endpoint(ordinal))
        })?;
        for (ordinal, path) in stream_paths.iter().enumerate() {
            *value_at_path_mut(&mut value, path)? = SchemaValue::U64(ordinal as u64);
        }
        if !foreign_streams.is_empty() {
            return Err("method input contains unreferenced foreign stream mappings".into());
        }
        let handles: Vec<_> = streams.iter().flatten().collect();
        if handles.iter().copied().collect::<HashSet<_>>().len() != handles.len() {
            return Err("method input references a foreign stream more than once".into());
        }
        Ok((
            Self {
                value,
                stream_paths,
                streams,
                external_rest: false,
            },
            transport_ids,
        ))
    }

    pub fn streams(&self) -> &[Option<DurableStreamHandle>] {
        &self.streams
    }

    /// Compares the captured value and stream ordinals, excluding foreign handle identities.
    pub fn same_unbound_value(&self, other: &Self) -> bool {
        self.value.bitwise_eq(&other.value)
            && self.stream_paths == other.stream_paths
            && self.external_rest == other.external_rest
    }

    /// Retains the external REST restriction on host-managed result capabilities.
    pub fn for_external_rest(mut self) -> Self {
        self.external_rest = true;
        self
    }

    pub fn is_external_rest(&self) -> bool {
        self.external_rest
    }

    pub fn decode(
        &self,
        mut stream: impl FnMut(usize, Option<&DurableStreamHandle>) -> Result<SchemaValueStream, String>,
    ) -> Result<SchemaValue, String> {
        if self.stream_paths.len() != self.streams.len() {
            return Err("stored method input stream mappings do not match its paths".into());
        }
        let mut value = self.value.clone();
        for (ordinal, (path, handle)) in self.stream_paths.iter().zip(&self.streams).enumerate() {
            let leaf = value_at_path_mut(&mut value, path)?;
            if !matches!(leaf, SchemaValue::U64(index) if *index == ordinal as u64) {
                return Err("stored method input stream path does not contain its ordinal".into());
            }
            *leaf = SchemaValue::Stream(stream(ordinal, handle.as_ref())?);
        }
        Ok(value)
    }
}

fn value_at_path_mut<'a>(
    mut value: &'a mut SchemaValue,
    path: &[StreamValuePathStep],
) -> Result<&'a mut SchemaValue, String> {
    use crate::schema::ResultValuePayload;
    for step in path {
        value =
            match (step, value) {
                (StreamValuePathStep::RecordField(index), SchemaValue::Record { fields }) => {
                    fields.get_mut(*index as usize)
                }
                (StreamValuePathStep::VariantCasePayload(index), SchemaValue::Variant(variant)) => {
                    if *index == variant.case {
                        variant.payload.as_deref_mut()
                    } else {
                        None
                    }
                }
                (StreamValuePathStep::TupleElement(index), SchemaValue::Tuple { elements })
                | (StreamValuePathStep::ListElement(index), SchemaValue::List { elements })
                | (
                    StreamValuePathStep::FixedListElement(index),
                    SchemaValue::FixedList { elements },
                ) => elements.get_mut(*index as usize),
                (StreamValuePathStep::MapEntry { index, side }, SchemaValue::Map { entries }) => {
                    entries
                        .get_mut(*index as usize)
                        .map(|(key, value)| match side {
                            StreamMapSide::Key => key,
                            StreamMapSide::Value => value,
                        })
                }
                (StreamValuePathStep::OptionSome, SchemaValue::Option { inner }) => {
                    inner.as_deref_mut()
                }
                (
                    StreamValuePathStep::ResultOk,
                    SchemaValue::Result(ResultValuePayload::Ok { value }),
                )
                | (
                    StreamValuePathStep::ResultErr,
                    SchemaValue::Result(ResultValuePayload::Err { value }),
                ) => value.as_deref_mut(),
                // Without a schema the recursive transport codec uses branch zero for unions.
                (StreamValuePathStep::UnionBranch(0), SchemaValue::Union(union)) => {
                    Some(union.body.as_mut())
                }
                _ => None,
            }
            .ok_or_else(|| {
                "stored method input stream path does not match its value".to_string()
            })?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::durable_stream::{StreamId, StreamInvocationId};
    use crate::model::component::ComponentRevision;
    use crate::model::environment::EnvironmentId;
    use crate::model::{AgentFingerprint, AgentId, InvocationId, OplogIndex};
    use crate::serialization::{deserialize, serialize};
    use golem_schema::proto::golem::schema::{
        RecordValue, SchemaValueStreamReference, schema_value,
    };
    use golem_schema::schema::SchemaFingerprintV1;
    use test_r::test;
    use uuid::Uuid;

    fn public_json(input: &AgentMethodInput) -> serde_json::Value {
        let value = input.to_public_value().unwrap();
        crate::schema::render::to_json_value(value.graph(), value.root_type(), value.value())
            .unwrap()
    }

    #[test]
    fn public_method_input_keeps_uninterpreted_json() {
        let raw = JsonMethodInput::capture(br#"{"rejected":1e400,"value":-0.0}"#).unwrap();
        assert_eq!(
            public_json(&AgentMethodInput::PublicJson(raw.clone())),
            serde_json::json!({"PublicJson": r#"{"rejected":1e400,"value":-0.0}"#})
        );
    }

    #[test]
    fn public_native_input_redacts_capabilities_and_preserves_bits_and_ordinals() {
        use crate::schema::{
            PermissionCardValuePayload, QuotaTokenValuePayload, SecretValuePayload,
        };

        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::F32(f32::from_bits(0x7fc0_002a)),
                SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0031)),
                SchemaValue::F64(-0.0),
                SchemaValue::Map {
                    entries: vec![(
                        SchemaValue::String("nested".into()),
                        SchemaValue::Option {
                            inner: Some(Box::new(SchemaValue::List {
                                elements: vec![
                                    SchemaValue::Secret(SecretValuePayload {
                                        secret_id: Uuid::from_u128(41),
                                        config_key: Some(vec!["private-config-key".into()]),
                                        version: 9,
                                        resolved_at: chrono::DateTime::UNIX_EPOCH,
                                        category: None,
                                    }),
                                    SchemaValue::QuotaToken(QuotaTokenValuePayload {
                                        environment_id: golem_schema::model::EnvironmentId::new(
                                            Uuid::from_u128(42),
                                        ),
                                        resource_name: "private-resource".into(),
                                        expected_use: 3,
                                        last_credit: 2,
                                        last_credit_at: chrono::DateTime::UNIX_EPOCH,
                                    }),
                                    SchemaValue::PermissionCard(PermissionCardValuePayload {
                                        card_id: Uuid::from_u128(43),
                                        parent_ids: vec![],
                                        expires_at: None,
                                        polymorphic: false,
                                    }),
                                ],
                            })),
                        },
                    )],
                },
            ],
        };
        let (input, _) = NativeMethodInput::capture(native(value, 93), vec![]).unwrap();
        let projected = AgentMethodInput::Native(input).to_public_value().unwrap();
        let PublicMethodInput::Native {
            value: decoded,
            stream_paths,
            streams,
        } = PublicMethodInput::from_value(projected.value()).unwrap()
        else {
            panic!("expected native public input")
        };
        assert_eq!(streams, vec![None]);
        assert_eq!(
            stream_paths,
            vec![vec![StreamValuePathStep::RecordField(1)]]
        );
        let SchemaValue::Record { fields } = decoded else {
            panic!("expected a record")
        };
        assert_eq!(fields[1], SchemaValue::U64(0));
        let SchemaValue::Record { fields } = &fields[0] else {
            panic!("expected nested data")
        };
        assert!(fields[0].bitwise_eq(&SchemaValue::F32(f32::from_bits(0x7fc0_002a))));
        assert!(fields[1].bitwise_eq(&SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0031))));
        assert!(fields[2].bitwise_eq(&SchemaValue::F64(-0.0)));
        assert_eq!(
            fields[3],
            SchemaValue::Map {
                entries: vec![(
                    SchemaValue::String("nested".into()),
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::List {
                            elements: vec![
                                SchemaValue::String("<redacted: secret>".into()),
                                SchemaValue::String("<redacted: quota-token>".into()),
                                SchemaValue::String("<redacted: permission-card>".into()),
                            ],
                        })),
                    }
                )],
            }
        );
    }

    fn native(value: SchemaValue, stream_id: u64) -> ProtoSchemaValue {
        ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![
                    value.try_into().unwrap(),
                    ProtoSchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id },
                        )),
                    },
                ],
            })),
        }
    }

    #[test]
    fn native_request_identity_preserves_float_bits_but_not_attachment_stream_ids() {
        for value in [
            SchemaValue::F32(f32::from_bits(0x7fc0_002a)),
            SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0031)),
            SchemaValue::F64(-0.0),
        ] {
            let (first, first_ids) =
                NativeMethodInput::capture(native(value.clone(), 93), vec![]).unwrap();
            let (retry, retry_ids) =
                NativeMethodInput::capture(native(value.clone(), 7), vec![]).unwrap();
            assert_eq!(first, retry);
            assert_eq!(first_ids, vec![93]);
            assert_eq!(retry_ids, vec![7]);
            let stored: NativeMethodInput = deserialize(&serialize(&first).unwrap()).unwrap();
            assert_eq!(first, stored);
            let decoded = stored
                .decode(|ordinal, handle| {
                    assert_eq!(ordinal, 0);
                    assert!(handle.is_none());
                    Ok(SchemaValueStream::from_host_endpoint(()))
                })
                .unwrap();
            let SchemaValue::Record { fields } = decoded else {
                panic!("expected a record")
            };
            assert!(fields[0].bitwise_eq(&value));
        }
        let (positive_zero, _) =
            NativeMethodInput::capture(native(SchemaValue::F64(0.0), 7), vec![]).unwrap();
        let (negative_zero, _) =
            NativeMethodInput::capture(native(SchemaValue::F64(-0.0), 7), vec![]).unwrap();
        assert_ne!(positive_zero, negative_zero);
        let (first_nan, _) = NativeMethodInput::capture(
            native(SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0031)), 7),
            vec![],
        )
        .unwrap();
        let (other_nan, _) = NativeMethodInput::capture(
            native(SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0032)), 7),
            vec![],
        )
        .unwrap();
        assert_ne!(first_nan, other_nan);
    }

    #[test]
    fn native_stream_positions_survive_storage_without_claiming_scalar_ordinals() {
        use crate::schema::streaming::encode_recursive_stream_value;
        use crate::schema::{ResultValuePayload, UnionValuePayload, VariantValuePayload};
        let stream = |id: u64| SchemaValue::Stream(SchemaValueStream::from_host_endpoint(id));
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(0),
                SchemaValue::Variant(VariantValuePayload {
                    case: 3,
                    payload: Some(Box::new(SchemaValue::Tuple {
                        elements: vec![stream(91), SchemaValue::U64(1)],
                    })),
                }),
                SchemaValue::Map {
                    entries: vec![(
                        stream(17),
                        SchemaValue::Option {
                            inner: Some(Box::new(SchemaValue::List {
                                elements: vec![SchemaValue::Result(ResultValuePayload::Ok {
                                    value: Some(Box::new(stream(58))),
                                })],
                            })),
                        },
                    )],
                },
                SchemaValue::Union(UnionValuePayload {
                    tag: "other".into(),
                    body: Box::new(SchemaValue::FixedList {
                        elements: vec![SchemaValue::Result(ResultValuePayload::Err {
                            value: Some(Box::new(stream(32))),
                        })],
                    }),
                }),
            ],
        };
        let original = encode_recursive_stream_value(&value, |stream, _| {
            stream.with_host_endpoint::<u64, _>(|id| *id)
        })
        .unwrap();
        let (input, ids) = NativeMethodInput::capture(original.clone(), vec![]).unwrap();
        assert_eq!(ids, vec![91, 17, 58, 32]);
        let stored: NativeMethodInput = deserialize(&serialize(&input).unwrap()).unwrap();
        let decoded = stored
            .decode(|ordinal, _| Ok(SchemaValueStream::from_host_endpoint(ids[ordinal])))
            .unwrap();
        let restored = encode_recursive_stream_value(&decoded, |stream, _| {
            stream.with_host_endpoint::<u64, _>(|id| *id)
        })
        .unwrap();
        assert_eq!(original, restored);
        let projected = AgentMethodInput::Native(stored).to_public_value().unwrap();
        let PublicMethodInput::Native {
            value,
            stream_paths,
            streams,
        } = PublicMethodInput::from_value(projected.value()).unwrap()
        else {
            panic!("expected native projection")
        };
        assert_eq!(stream_paths.len(), 4);
        assert_eq!(streams, vec![None; 4]);
        let SchemaValue::Record { fields } = value else {
            panic!("expected record")
        };
        assert_eq!(fields[0], SchemaValue::U64(0));
    }

    #[test]
    fn native_request_identity_restores_the_full_structural_depth() {
        for (map, depths) in [(false, [49, 50, 51, 128]), (true, [32, 33, 34, 128])] {
            for depth in depths {
                let value = (0..depth).fold(SchemaValue::U32(71), |inner, _| {
                    if map {
                        SchemaValue::Map {
                            entries: vec![(SchemaValue::String("key".into()), inner)],
                        }
                    } else {
                        SchemaValue::Option {
                            inner: Some(Box::new(inner)),
                        }
                    }
                });
                let (input, streams) =
                    NativeMethodInput::capture(value.clone().try_into().unwrap(), vec![]).unwrap();
                assert!(streams.is_empty());
                let stored: NativeMethodInput = deserialize(&serialize(&input).unwrap()).unwrap();
                let restored = stored.decode(|_, _| panic!("no streams expected")).unwrap();
                assert!(restored.bitwise_eq(&value), "map={map}, depth={depth}");
                AgentMethodInput::Native(stored).to_public_value().unwrap();
            }
        }
        let too_deep = (0..129).fold(SchemaValue::U32(71), |inner, _| SchemaValue::Option {
            inner: Some(Box::new(inner)),
        });
        assert!(NativeMethodInput::capture(too_deep.try_into().unwrap(), vec![]).is_err());
    }

    #[test]
    fn native_request_identity_retains_foreign_stream_identity_after_storage() {
        let source_invocation = StreamInvocationId {
            callee_environment_id: EnvironmentId(Uuid::from_u128(1)),
            callee: AgentId::from_parts(7, 2),
            callee_fingerprint: AgentFingerprint(Uuid::from_u128(3)),
            idempotency_key: InvocationId(Uuid::from_u128(4)),
        };
        let handle = DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(Uuid::from_u128(5)),
            producer_environment_id: source_invocation.callee_environment_id,
            producer: source_invocation.callee,
            expected_producer_fingerprint: source_invocation.callee_fingerprint,
            producer_generation: OplogIndex::from_u64(6),
            source_invocation,
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([17; 32]),
        };
        let (original, _) = NativeMethodInput::capture(
            native(SchemaValue::U32(11), 93),
            vec![(93, handle.clone())],
        )
        .unwrap();
        let input = AgentMethodInput::Native(original.clone());
        let stored: AgentMethodInput = deserialize(&serialize(&input).unwrap()).unwrap();
        assert_eq!(input, stored);
        let projected = public_json(&stored);
        assert_eq!(
            projected["Native"]["streams"][0]["producer"]["uuid"],
            handle.producer.to_string()
        );
        assert_eq!(
            projected["Native"]["streams"][0]["expected_producer_fingerprint"]["uuid"],
            handle.expected_producer_fingerprint.0.to_string()
        );
        assert_eq!(
            projected["Native"]["streams"][0]["producer_generation"],
            "6"
        );
        let (retry, _) =
            NativeMethodInput::capture(native(SchemaValue::U32(11), 7), vec![(7, handle.clone())])
                .unwrap();
        assert_eq!(original, retry);
        let mut other_handle = handle.clone();
        other_handle.expected_producer_fingerprint = AgentFingerprint(Uuid::from_u128(8));
        let (other, _) =
            NativeMethodInput::capture(native(SchemaValue::U32(11), 7), vec![(7, other_handle)])
                .unwrap();
        assert_ne!(original, other);
        assert!(
            NativeMethodInput::capture(native(SchemaValue::U32(11), 7), vec![(93, handle)])
                .is_err()
        );
    }

    #[test]
    fn json_request_identity_sorts_objects_without_coercing_numbers() {
        let first = JsonMethodInput::capture(br#"{ "z": [2, 3], "a": {"b": 1.0} }"#).unwrap();
        let retry = JsonMethodInput::capture(br#"{"a":{"b":1.0},"z":[2,3]}"#).unwrap();
        let integer = JsonMethodInput::capture(br#"{"a":{"b":1},"z":[2,3]}"#).unwrap();
        assert_eq!(first, retry);
        assert_ne!(first, integer);
        let precise =
            JsonMethodInput::capture(br#"{"z":[2,3],"a":{"b":1.00000000000000001}}"#).unwrap();
        assert_ne!(first, precise);
        assert_eq!(
            precise.as_str(),
            r#"{"a":{"b":1.00000000000000001},"z":[2,3]}"#
        );
        let large = JsonMethodInput::capture(br#"{"z":1e400,"a":-1e999}"#).unwrap();
        assert_eq!(large.as_str(), r#"{"a":-1e999,"z":1e400}"#);
        let stored: JsonMethodInput = deserialize(&serialize(&large).unwrap()).unwrap();
        assert_eq!(large, stored);
        assert!(JsonMethodInput::capture(br#"{"a":1,"a":2}"#).is_err());
        assert!(JsonMethodInput::capture(br#"{"a":1,"\u0061":2}"#).is_err());
        assert!(JsonMethodInput::capture(br#"{} []"#).is_err());
        let at_limit = format!("{}0{}", "[".repeat(64), "]".repeat(64));
        assert!(JsonMethodInput::capture(at_limit.as_bytes()).is_ok());
        let too_deep = format!("[{at_limit}]");
        assert!(JsonMethodInput::capture(too_deep.as_bytes()).is_err());
        let input = AgentMethodInput::PublicJson(first);
        let stored: AgentMethodInput = deserialize(&serialize(&input).unwrap()).unwrap();
        assert_eq!(input, stored);
    }
}
