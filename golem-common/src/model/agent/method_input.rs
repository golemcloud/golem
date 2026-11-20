// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use crate::base_model::durable_stream::DurableStreamHandle;
pub use crate::model::invocation_session_public::JsonMethodInput;
use crate::model::invocation_session_public::{MAX_COLLECTION_SIZE, MAX_LOGICAL_VALUE_SIZE};
use crate::schema::streaming::{
    decode_recursive_stream_value, preflight_proto_recursive_stream_value,
    remap_recursive_stream_references,
};
use crate::schema::{SchemaValue, stream::SchemaValueStream};
use desert_rust::BinaryCodec;
use golem_schema::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Debug, Formatter};

/// The original method input, before interpreting it against an execution schema.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentMethodInput {
    Native(NativeMethodInput),
    PublicJson(JsonMethodInput),
    Http(HttpMethodInput),
}

/// Schema-native data with request-local stream ordinals instead of transport endpoints.
#[derive(Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct NativeMethodInput {
    value: Vec<u8>,
    streams: Vec<Option<DurableStreamHandle>>,
}

impl Debug for NativeMethodInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMethodInput")
            .field("bytes", &self.value.len())
            .field("streams", &self.streams.len())
            .finish()
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
        let value = remap_recursive_stream_references(input, |transport_id, _| {
            let handle = foreign_streams.remove(&transport_id);
            if has_foreign_streams && handle.is_none() {
                return Err("method input references an unmapped foreign stream".into());
            }
            let ordinal = streams.len() as u64;
            streams.push(handle);
            Ok(ordinal)
        })?;
        if !foreign_streams.is_empty() {
            return Err("method input contains unreferenced foreign stream mappings".into());
        }
        let handles: Vec<_> = streams.iter().flatten().collect();
        if handles.iter().copied().collect::<HashSet<_>>().len() != handles.len() {
            return Err("method input references a foreign stream more than once".into());
        }
        Ok((
            Self {
                value: crate::serialization::serialize(&value)?,
                streams,
            },
            transport_ids,
        ))
    }

    pub fn streams(&self) -> &[Option<DurableStreamHandle>] {
        &self.streams
    }

    pub fn decode(
        &self,
        mut stream: impl FnMut(usize, Option<&DurableStreamHandle>) -> Result<SchemaValueStream, String>,
    ) -> Result<SchemaValue, String> {
        let value: ProtoSchemaValue = crate::serialization::decode(&self.value)
            .map_err(|error| format!("invalid stored method input: {error}"))?;
        decode_recursive_stream_value(value, |ordinal, _| {
            let ordinal = usize::try_from(ordinal)
                .map_err(|_| "stored method input stream ordinal is out of range".to_string())?;
            let handle = self
                .streams
                .get(ordinal)
                .ok_or_else(|| "stored method input stream ordinal is out of range".to_string())?;
            stream(ordinal, handle.as_ref())
        })
    }
}

/// A captured route extraction contract and its raw values, without a method schema.
#[derive(Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct HttpMethodInput {
    arguments: Vec<HttpMethodArgument>,
    body: HttpMethodBody,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct HttpMethodArgument {
    pub argument_name: String,
    pub source: HttpMethodArgumentSource,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum HttpMethodArgumentSource {
    Path {
        path_segment_index: u32,
        value: String,
    },
    Query {
        parameter_name: String,
        entries: BTreeMap<String, Vec<String>>,
    },
    Header {
        header_name: String,
        values: Vec<String>,
    },
    JsonProperty {
        property_name: String,
    },
    BinaryBody,
    TextBody,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum HttpMethodBody {
    Absent,
    Json(JsonMethodInput),
    Binary {
        value: Vec<u8>,
        mime_type: Option<String>,
    },
    Text {
        value: String,
        language: Option<String>,
    },
}

impl Debug for HttpMethodInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpMethodInput")
            .field("arguments", &self.arguments.len())
            .finish_non_exhaustive()
    }
}

impl HttpMethodInput {
    pub fn capture(
        mut arguments: Vec<HttpMethodArgument>,
        body: HttpMethodBody,
    ) -> Result<Self, String> {
        if arguments.len() > MAX_COLLECTION_SIZE {
            return Err("HTTP method input contains too many argument bindings".into());
        }
        arguments.sort_by(|a, b| a.argument_name.cmp(&b.argument_name));
        if arguments
            .iter()
            .any(|argument| argument.argument_name.is_empty())
            || arguments
                .windows(2)
                .any(|pair| pair[0].argument_name == pair[1].argument_name)
        {
            return Err("HTTP method input requires unique, nonempty argument names".into());
        }
        let mut bytes = 0usize;
        let mut items = arguments.len();
        let mut add = |size: usize| {
            bytes = bytes
                .checked_add(size)
                .filter(|bytes| *bytes <= MAX_LOGICAL_VALUE_SIZE)
                .ok_or_else(|| {
                    "HTTP method input exceeds the 16 MiB logical value limit".to_string()
                })?;
            Ok::<_, String>(())
        };
        for argument in &arguments {
            add(argument.argument_name.len())?;
            match &argument.source {
                HttpMethodArgumentSource::Path { value, .. } => add(value.len())?,
                HttpMethodArgumentSource::Query {
                    parameter_name,
                    entries,
                } => {
                    add(parameter_name.len())?;
                    items = items.saturating_add(entries.len());
                    for (name, values) in entries {
                        add(name.len())?;
                        items = items.saturating_add(values.len());
                        for value in values {
                            add(value.len())?;
                        }
                    }
                }
                HttpMethodArgumentSource::Header {
                    header_name,
                    values,
                } => {
                    add(header_name.len())?;
                    items = items.saturating_add(values.len());
                    for value in values {
                        add(value.len())?;
                    }
                }
                HttpMethodArgumentSource::JsonProperty { property_name } => {
                    add(property_name.len())?
                }
                HttpMethodArgumentSource::BinaryBody | HttpMethodArgumentSource::TextBody => {}
            }
        }
        if items > MAX_COLLECTION_SIZE {
            return Err("HTTP method input contains too many captured values".into());
        }
        match &body {
            HttpMethodBody::Absent => {}
            HttpMethodBody::Json(value) => add(value.as_str().len())?,
            HttpMethodBody::Binary { value, mime_type } => {
                add(value.len())?;
                add(mime_type.as_deref().map_or(0, str::len))?;
            }
            HttpMethodBody::Text { value, language } => {
                add(value.len())?;
                add(language.as_deref().map_or(0, str::len))?;
            }
        }
        Ok(Self { arguments, body })
    }

    pub fn arguments(&self) -> &[HttpMethodArgument] {
        &self.arguments
    }

    pub fn body(&self) -> &HttpMethodBody {
        &self.body
    }
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
            let decoded = first
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

    #[test]
    fn http_request_identity_retains_named_raw_sources_and_presence() {
        let query = HttpMethodArgument {
            argument_name: "count".into(),
            source: HttpMethodArgumentSource::Query {
                parameter_name: "wire".into(),
                entries: BTreeMap::from([("wire".into(), vec!["0007".into(), "9".into()])]),
            },
        };
        let text = HttpMethodArgument {
            argument_name: "message".into(),
            source: HttpMethodArgumentSource::JsonProperty {
                property_name: "text".into(),
            },
        };
        let body = HttpMethodBody::Json(JsonMethodInput::capture(br#"{"text":"hello"}"#).unwrap());
        let first =
            HttpMethodInput::capture(vec![text.clone(), query.clone()], body.clone()).unwrap();
        let retry =
            HttpMethodInput::capture(vec![query.clone(), text.clone()], body.clone()).unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.arguments()[0], query);
        let input = AgentMethodInput::Http(first);
        let stored: AgentMethodInput = deserialize(&serialize(&input).unwrap()).unwrap();
        assert_eq!(input, stored);
        let mut absent = query.clone();
        let HttpMethodArgumentSource::Query { entries, .. } = &mut absent.source else {
            unreachable!()
        };
        entries.clear();
        let absent = HttpMethodInput::capture(vec![absent, text.clone()], body.clone()).unwrap();
        assert_ne!(retry, absent);
        assert!(HttpMethodInput::capture(vec![text.clone(), text], body).is_err());
        assert!(
            HttpMethodInput::capture(
                vec![query],
                HttpMethodBody::Binary {
                    value: vec![0; MAX_LOGICAL_VALUE_SIZE],
                    mime_type: None,
                },
            )
            .is_err()
        );
    }
}
