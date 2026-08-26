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

use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::schema::{
    FixedListValue, ListValue, MapEntry, MapValue, OptionValue, RecordValue, ResultValue,
    SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, TupleValue, UnionValue,
    VariantValue, result_value as proto_result_value, schema_value as proto_schema_value,
};
use golem_common::base_model::durable_stream::{
    MAX_DURABLE_STREAM_ITEM_SIZE, MAX_NEW_STREAM_HANDLES_PER_VALUE,
    MAX_STREAM_VALUE_TRAVERSAL_DEPTH, StreamMapSideV1, StreamValuePathStepV1,
};
use golem_schema::schema::{SchemaGraph, SchemaType, SchemaValue, SchemaValueStream};
use prost::Message;

fn push_recursive_path(
    path: &mut Vec<StreamValuePathStepV1>,
    step: StreamValuePathStepV1,
) -> Result<(), String> {
    if path.len() >= MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
        return Err(
            "ResourceExhausted: recursive stream value exceeds the traversal depth limit"
                .to_string(),
        );
    }
    path.push(step);
    Ok(())
}

fn union_branch_index(
    graph: &SchemaGraph,
    root: &SchemaType,
    path: &[StreamValuePathStepV1],
    tag: &str,
) -> Result<u32, String> {
    let current = schema_type_at_path(graph, root, path)?;
    let SchemaType::Union { spec, .. } = current else {
        return Err("union value path does not match the pinned schema".to_string());
    };
    let index = spec
        .branches
        .iter()
        .position(|branch| branch.tag == tag)
        .ok_or_else(|| format!("union branch tag {tag:?} is not present in the pinned schema"))?;
    u32::try_from(index).map_err(|_| "union branch index does not fit in u32".to_string())
}

fn schema_type_at_path<'a>(
    graph: &'a SchemaGraph,
    root: &'a SchemaType,
    path: &[StreamValuePathStepV1],
) -> Result<&'a SchemaType, String> {
    let mut current = root;
    for step in path {
        current = graph
            .resolve_ref(current)
            .map_err(|error| error.to_string())?;
        current = match (step, current) {
            (StreamValuePathStepV1::RecordField(index), SchemaType::Record { fields, .. }) => {
                &fields
                    .get(*index as usize)
                    .ok_or_else(|| "stream record path is out of range".to_string())?
                    .body
            }
            (
                StreamValuePathStepV1::VariantCasePayload(index),
                SchemaType::Variant { cases, .. },
            ) => cases
                .get(*index as usize)
                .and_then(|case| case.payload.as_ref())
                .ok_or_else(|| "stream variant path has no payload".to_string())?,
            (StreamValuePathStepV1::TupleElement(index), SchemaType::Tuple { elements, .. }) => {
                elements
                    .get(*index as usize)
                    .ok_or_else(|| "stream tuple path is out of range".to_string())?
            }
            (StreamValuePathStepV1::ListElement(_), SchemaType::List { element, .. })
            | (StreamValuePathStepV1::FixedListElement(_), SchemaType::FixedList { element, .. }) => {
                element
            }
            (
                StreamValuePathStepV1::MapEntry {
                    side: StreamMapSideV1::Key,
                    ..
                },
                SchemaType::Map { key, .. },
            ) => key,
            (
                StreamValuePathStepV1::MapEntry {
                    side: StreamMapSideV1::Value,
                    ..
                },
                SchemaType::Map { value, .. },
            ) => value,
            (StreamValuePathStepV1::OptionSome, SchemaType::Option { inner, .. }) => inner,
            (StreamValuePathStepV1::ResultOk, SchemaType::Result { spec, .. }) => spec
                .ok
                .as_deref()
                .ok_or_else(|| "stream result ok path has no payload".to_string())?,
            (StreamValuePathStepV1::ResultErr, SchemaType::Result { spec, .. }) => spec
                .err
                .as_deref()
                .ok_or_else(|| "stream result error path has no payload".to_string())?,
            (StreamValuePathStepV1::UnionBranch(index), SchemaType::Union { spec, .. }) => {
                &spec
                    .branches
                    .get(*index as usize)
                    .ok_or_else(|| "stream union path is out of range".to_string())?
                    .body
            }
            _ => return Err("stream value path does not match the pinned schema".to_string()),
        };
    }
    graph
        .resolve_ref(current)
        .map_err(|error| error.to_string())
}

pub(crate) fn decode_recursive_stream_value(
    value: ProtoSchemaValue,
    mut stream: impl FnMut(u64, &[StreamValuePathStepV1]) -> Result<SchemaValueStream, String>,
) -> Result<SchemaValue, String> {
    decode_recursive_stream_value_inner(value, None, &mut stream)
}

pub(crate) fn decode_recursive_stream_value_with_schema(
    value: ProtoSchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
    mut stream: impl FnMut(u64, &[StreamValuePathStepV1]) -> Result<SchemaValueStream, String>,
) -> Result<SchemaValue, String> {
    decode_recursive_stream_value_inner(value, Some((graph, root)), &mut stream)
}

fn decode_recursive_stream_value_inner(
    value: ProtoSchemaValue,
    schema: Option<(&SchemaGraph, &SchemaType)>,
    stream: &mut impl FnMut(u64, &[StreamValuePathStepV1]) -> Result<SchemaValueStream, String>,
) -> Result<SchemaValue, String> {
    fn decode(
        value: ProtoSchemaValue,
        schema: Option<(&SchemaGraph, &SchemaType)>,
        path: &mut Vec<StreamValuePathStepV1>,
        stream: &mut impl FnMut(u64, &[StreamValuePathStepV1]) -> Result<SchemaValueStream, String>,
    ) -> Result<SchemaValue, String> {
        let value = value
            .value
            .ok_or_else(|| "schema value has no value".to_string())?;
        match value {
            proto_schema_value::Value::StreamReference(reference) => {
                stream(reference.stream_id, path).map(SchemaValue::Stream)
            }
            proto_schema_value::Value::RecordValue(value) => Ok(SchemaValue::Record {
                fields: value
                    .fields
                    .into_iter()
                    .enumerate()
                    .map(|(index, field)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::RecordField(index as u32),
                        )?;
                        let result = decode(field, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::VariantValue(value) => Ok(SchemaValue::Variant(
                golem_schema::schema::schema_value::VariantValuePayload {
                    case: value.case,
                    payload: value
                        .payload
                        .map(|payload| {
                            push_recursive_path(
                                path,
                                StreamValuePathStepV1::VariantCasePayload(value.case),
                            )?;
                            let result = decode(*payload, schema, path, stream).map(Box::new);
                            path.pop();
                            result
                        })
                        .transpose()?,
                },
            )),
            proto_schema_value::Value::TupleValue(value) => Ok(SchemaValue::Tuple {
                elements: value
                    .elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::TupleElement(index as u32),
                        )?;
                        let result = decode(element, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::ListValue(value) => Ok(SchemaValue::List {
                elements: value
                    .elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::ListElement(index as u32),
                        )?;
                        let result = decode(element, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::FixedListValue(value) => Ok(SchemaValue::FixedList {
                elements: value
                    .elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::FixedListElement(index as u32),
                        )?;
                        let result = decode(element, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::MapValue(value) => Ok(SchemaValue::Map {
                entries: value
                    .entries
                    .into_iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::MapEntry {
                                index: index as u32,
                                side: StreamMapSideV1::Key,
                            },
                        )?;
                        let key = decode(
                            entry
                                .key
                                .ok_or_else(|| "schema map entry has no key".to_string())?,
                            schema,
                            path,
                            stream,
                        );
                        path.pop();
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::MapEntry {
                                index: index as u32,
                                side: StreamMapSideV1::Value,
                            },
                        )?;
                        let value = decode(
                            entry
                                .value
                                .ok_or_else(|| "schema map entry has no value".to_string())?,
                            schema,
                            path,
                            stream,
                        );
                        path.pop();
                        Ok((key?, value?))
                    })
                    .collect::<Result<_, String>>()?,
            }),
            proto_schema_value::Value::OptionValue(value) => Ok(SchemaValue::Option {
                inner: value
                    .inner
                    .map(|inner| {
                        push_recursive_path(path, StreamValuePathStepV1::OptionSome)?;
                        let result = decode(*inner, schema, path, stream).map(Box::new);
                        path.pop();
                        result
                    })
                    .transpose()?,
            }),
            proto_schema_value::Value::ResultValue(value) => {
                let result = match value
                    .result
                    .ok_or_else(|| "result value has no result arm".to_string())?
                {
                    proto_result_value::Result::Ok(value) => {
                        push_recursive_path(path, StreamValuePathStepV1::ResultOk)?;
                        let value = decode(*value, schema, path, stream).map(Box::new);
                        path.pop();
                        golem_schema::schema::schema_value::ResultValuePayload::Ok {
                            value: Some(value?),
                        }
                    }
                    proto_result_value::Result::Err(value) => {
                        push_recursive_path(path, StreamValuePathStepV1::ResultErr)?;
                        let value = decode(*value, schema, path, stream).map(Box::new);
                        path.pop();
                        golem_schema::schema::schema_value::ResultValuePayload::Err {
                            value: Some(value?),
                        }
                    }
                    proto_result_value::Result::OkUnit(_) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Ok { value: None }
                    }
                    proto_result_value::Result::ErrUnit(_) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Err { value: None }
                    }
                };
                Ok(SchemaValue::Result(result))
            }
            proto_schema_value::Value::UnionValue(value) => {
                let branch_index = match schema {
                    Some((graph, root)) => union_branch_index(graph, root, path, &value.tag)?,
                    None => 0,
                };
                push_recursive_path(path, StreamValuePathStepV1::UnionBranch(branch_index))?;
                let body = decode(
                    *value
                        .body
                        .ok_or_else(|| "schema union value has no body".to_string())?,
                    schema,
                    path,
                    stream,
                );
                path.pop();
                Ok(SchemaValue::Union(
                    golem_schema::schema::schema_value::UnionValuePayload {
                        tag: value.tag,
                        body: Box::new(body?),
                    },
                ))
            }
            value => ProtoSchemaValue { value: Some(value) }.try_into(),
        }
    }

    decode(value, schema, &mut Vec::new(), stream)
}

pub(crate) fn encode_recursive_stream_value(
    value: &SchemaValue,
    mut stream: impl FnMut(&SchemaValueStream, &[StreamValuePathStepV1]) -> Result<u64, String>,
) -> Result<ProtoSchemaValue, String> {
    encode_recursive_stream_value_inner(value, None, &mut stream)
}

pub(crate) fn encode_recursive_stream_value_with_schema(
    value: &SchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
    mut stream: impl FnMut(&SchemaValueStream, &[StreamValuePathStepV1]) -> Result<u64, String>,
) -> Result<ProtoSchemaValue, String> {
    encode_recursive_stream_value_inner(value, Some((graph, root)), &mut stream)
}

fn encode_recursive_stream_value_inner(
    value: &SchemaValue,
    schema: Option<(&SchemaGraph, &SchemaType)>,
    stream: &mut impl FnMut(&SchemaValueStream, &[StreamValuePathStepV1]) -> Result<u64, String>,
) -> Result<ProtoSchemaValue, String> {
    fn encode(
        value: &SchemaValue,
        schema: Option<(&SchemaGraph, &SchemaType)>,
        path: &mut Vec<StreamValuePathStepV1>,
        stream: &mut impl FnMut(&SchemaValueStream, &[StreamValuePathStepV1]) -> Result<u64, String>,
    ) -> Result<ProtoSchemaValue, String> {
        let value = match value {
            SchemaValue::Stream(value) => {
                proto_schema_value::Value::StreamReference(SchemaValueStreamReference {
                    stream_id: stream(value, path)?,
                })
            }
            SchemaValue::Record { fields } => proto_schema_value::Value::RecordValue(RecordValue {
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::RecordField(index as u32),
                        )?;
                        let result = encode(field, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::Variant(value) => {
                let payload = value
                    .payload
                    .as_deref()
                    .map(|payload| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::VariantCasePayload(value.case),
                        )?;
                        let result = encode(payload, schema, path, stream).map(Box::new);
                        path.pop();
                        result
                    })
                    .transpose()?;
                proto_schema_value::Value::VariantValue(Box::new(VariantValue {
                    case: value.case,
                    payload,
                }))
            }
            SchemaValue::Tuple { elements } => proto_schema_value::Value::TupleValue(TupleValue {
                elements: elements
                    .iter()
                    .enumerate()
                    .map(|(index, element)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::TupleElement(index as u32),
                        )?;
                        let result = encode(element, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::List { elements } => proto_schema_value::Value::ListValue(ListValue {
                elements: elements
                    .iter()
                    .enumerate()
                    .map(|(index, element)| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::ListElement(index as u32),
                        )?;
                        let result = encode(element, schema, path, stream);
                        path.pop();
                        result
                    })
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::FixedList { elements } => {
                proto_schema_value::Value::FixedListValue(FixedListValue {
                    elements: elements
                        .iter()
                        .enumerate()
                        .map(|(index, element)| {
                            push_recursive_path(
                                path,
                                StreamValuePathStepV1::FixedListElement(index as u32),
                            )?;
                            let result = encode(element, schema, path, stream);
                            path.pop();
                            result
                        })
                        .collect::<Result<_, _>>()?,
                })
            }
            SchemaValue::Map { entries } => proto_schema_value::Value::MapValue(MapValue {
                entries: entries
                    .iter()
                    .enumerate()
                    .map(|(index, (key, value))| {
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::MapEntry {
                                index: index as u32,
                                side: StreamMapSideV1::Key,
                            },
                        )?;
                        let key = encode(key, schema, path, stream)?;
                        path.pop();
                        push_recursive_path(
                            path,
                            StreamValuePathStepV1::MapEntry {
                                index: index as u32,
                                side: StreamMapSideV1::Value,
                            },
                        )?;
                        let value = encode(value, schema, path, stream)?;
                        path.pop();
                        Ok(MapEntry {
                            key: Some(key),
                            value: Some(value),
                        })
                    })
                    .collect::<Result<_, String>>()?,
            }),
            SchemaValue::Option { inner } => {
                let inner = inner
                    .as_deref()
                    .map(|inner| {
                        push_recursive_path(path, StreamValuePathStepV1::OptionSome)?;
                        let result = encode(inner, schema, path, stream).map(Box::new);
                        path.pop();
                        result
                    })
                    .transpose()?;
                proto_schema_value::Value::OptionValue(Box::new(OptionValue { inner }))
            }
            SchemaValue::Result(result) => {
                let result = match result {
                    golem_schema::schema::schema_value::ResultValuePayload::Ok { value } => {
                        match value.as_deref() {
                            Some(value) => {
                                push_recursive_path(path, StreamValuePathStepV1::ResultOk)?;
                                let value = encode(value, schema, path, stream).map(Box::new)?;
                                path.pop();
                                proto_result_value::Result::Ok(value)
                            }
                            None => proto_result_value::Result::OkUnit(Empty {}),
                        }
                    }
                    golem_schema::schema::schema_value::ResultValuePayload::Err { value } => {
                        match value.as_deref() {
                            Some(value) => {
                                push_recursive_path(path, StreamValuePathStepV1::ResultErr)?;
                                let value = encode(value, schema, path, stream).map(Box::new)?;
                                path.pop();
                                proto_result_value::Result::Err(value)
                            }
                            None => proto_result_value::Result::ErrUnit(Empty {}),
                        }
                    }
                };
                proto_schema_value::Value::ResultValue(Box::new(ResultValue {
                    result: Some(result),
                }))
            }
            SchemaValue::Union(value) => {
                let branch_index = match schema {
                    Some((graph, root)) => union_branch_index(graph, root, path, &value.tag)?,
                    None => 0,
                };
                push_recursive_path(path, StreamValuePathStepV1::UnionBranch(branch_index))?;
                let body = encode(&value.body, schema, path, stream).map(Box::new)?;
                path.pop();
                proto_schema_value::Value::UnionValue(Box::new(UnionValue {
                    tag: value.tag.clone(),
                    body: Some(body),
                }))
            }
            _ => return value.clone().try_into(),
        };
        Ok(ProtoSchemaValue { value: Some(value) })
    }

    encode(value, schema, &mut Vec::new(), stream)
}

pub(crate) fn preflight_recursive_stream_value(
    value: &SchemaValue,
) -> Result<ProtoSchemaValue, String> {
    let mut stream_count = 0usize;
    let encoded = encode_recursive_stream_value(value, |_, _| {
        let stream_id = u64::try_from(stream_count)
            .map_err(|_| "durable stream handle index overflow".to_string())?;
        stream_count = stream_count
            .checked_add(1)
            .ok_or_else(|| "durable stream handle count overflow".to_string())?;
        Ok(stream_id)
    })?;
    if encoded.encoded_len() > MAX_DURABLE_STREAM_ITEM_SIZE {
        return Err(
            "ResourceExhausted: recursive stream value exceeds the 16 MiB logical value limit"
                .to_string(),
        );
    }
    if stream_count > MAX_NEW_STREAM_HANDLES_PER_VALUE {
        return Err(
            "ResourceExhausted: recursive stream value materializes more than 256 streams"
                .to_string(),
        );
    }
    Ok(encoded)
}

pub(crate) fn preflight_proto_recursive_stream_value(
    value: &ProtoSchemaValue,
) -> Result<Vec<u64>, String> {
    if value.encoded_len() > MAX_DURABLE_STREAM_ITEM_SIZE {
        return Err(
            "ResourceExhausted: recursive stream value exceeds the 16 MiB logical value limit"
                .to_string(),
        );
    }
    let mut stream_references = Vec::new();
    decode_recursive_stream_value(value.clone(), |stream_id, _| {
        stream_references.push(stream_id);
        Ok(SchemaValueStream::from_host_endpoint(()))
    })?;
    if stream_references.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
        return Err(
            "ResourceExhausted: recursive stream value materializes more than 256 streams"
                .to_string(),
        );
    }
    Ok(stream_references)
}

pub(crate) fn remap_recursive_stream_references(
    value: ProtoSchemaValue,
    mut remap: impl FnMut(u64, &[StreamValuePathStepV1]) -> Result<u64, String>,
) -> Result<ProtoSchemaValue, String> {
    let value = decode_recursive_stream_value(value, |stream_id, _| {
        Ok(SchemaValueStream::from_host_endpoint(stream_id))
    })?;
    encode_recursive_stream_value(&value, |stream, path| {
        stream
            .with_host_endpoint::<u64, _>(|stream_id| *stream_id)
            .and_then(|stream_id| remap(stream_id, path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_schema::schema::schema_value::UnionValuePayload;
    use golem_schema::schema::{
        DiscriminatorRule, FieldDiscriminator, NamedFieldType, UnionBranch, UnionSpec,
    };
    use test_r::test;

    fn union_with_stream_in_second_branch() -> SchemaType {
        let field = |name: &str, body| NamedFieldType {
            name: name.to_string(),
            body,
            metadata: Default::default(),
        };
        SchemaType::union(UnionSpec {
            branches: vec![
                UnionBranch {
                    tag: "plain".to_string(),
                    body: SchemaType::record(vec![field("kind", SchemaType::string())]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("plain".to_string()),
                    }),
                    metadata: Default::default(),
                },
                UnionBranch {
                    tag: "stream".to_string(),
                    body: SchemaType::record(vec![
                        field("kind", SchemaType::string()),
                        field("values", SchemaType::stream(Some(SchemaType::u32()))),
                    ]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("stream".to_string()),
                    }),
                    metadata: Default::default(),
                },
            ],
        })
    }

    #[test]
    fn schema_aware_recursive_walker_uses_the_union_branch_tag_as_the_path_index() {
        let root = union_with_stream_in_second_branch();
        let graph = SchemaGraph::anonymous(root.clone());
        let value = SchemaValue::Union(UnionValuePayload {
            tag: "stream".to_string(),
            body: Box::new(SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("stream".to_string()),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(())),
                ],
            }),
        });
        let expected_path = vec![
            StreamValuePathStepV1::UnionBranch(1),
            StreamValuePathStepV1::RecordField(1),
        ];

        let encoded =
            encode_recursive_stream_value_with_schema(&value, &graph, &root, |_, path| {
                assert_eq!(path, expected_path);
                Ok(17)
            })
            .unwrap();
        let decoded =
            decode_recursive_stream_value_with_schema(encoded, &graph, &root, |stream_id, path| {
                assert_eq!(stream_id, 17);
                assert_eq!(path, expected_path);
                Ok(SchemaValueStream::from_host_endpoint(()))
            })
            .unwrap();

        let SchemaValue::Union(decoded) = decoded else {
            panic!("expected a union value")
        };
        assert_eq!(decoded.tag, "stream");

        let plain = SchemaValue::Union(UnionValuePayload {
            tag: "plain".to_string(),
            body: Box::new(SchemaValue::Record {
                fields: vec![SchemaValue::String("plain".to_string())],
            }),
        });
        encode_recursive_stream_value_with_schema(&plain, &graph, &root, |_, _| {
            Err("the non-stream branch must not contain a stream".to_string())
        })
        .unwrap();
    }

    fn nested_option_value(depth: usize) -> SchemaValue {
        (0..depth).fold(SchemaValue::U32(1), |inner, _| SchemaValue::Option {
            inner: Some(Box::new(inner)),
        })
    }

    fn nested_proto_option_value(depth: usize) -> ProtoSchemaValue {
        (0..depth).fold(
            ProtoSchemaValue {
                value: Some(proto_schema_value::Value::U32Value(1)),
            },
            |inner, _| ProtoSchemaValue {
                value: Some(proto_schema_value::Value::OptionValue(Box::new(
                    OptionValue {
                        inner: Some(Box::new(inner)),
                    },
                ))),
            },
        )
    }

    #[test]
    fn recursive_value_preflight_enforces_the_complete_value_depth() {
        assert!(
            preflight_recursive_stream_value(&nested_option_value(
                MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            ))
            .is_ok()
        );
        assert!(
            preflight_recursive_stream_value(&nested_option_value(
                MAX_STREAM_VALUE_TRAVERSAL_DEPTH + 1
            ))
            .unwrap_err()
            .contains("ResourceExhausted")
        );
        assert!(
            preflight_proto_recursive_stream_value(&nested_proto_option_value(
                MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            ))
            .is_ok()
        );
        assert!(
            preflight_proto_recursive_stream_value(&nested_proto_option_value(
                MAX_STREAM_VALUE_TRAVERSAL_DEPTH + 1
            ))
            .unwrap_err()
            .contains("ResourceExhausted")
        );
    }
}
