// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::parameter_parsing::{
    parse_path_segment_value, parse_query_or_header_value, parse_query_parameter,
    schema_type_to_path_segment_type, schema_type_to_query_or_header_type,
};
use golem_common::model::agent::{HttpMethodArgumentSource, HttpMethodBody, HttpMethodInput};
use golem_common::model::invocation_session_public::parse_strict_json;
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::unstructured::wrap_unstructured_inline_for_schema;
use golem_common::schema::validation::validate_value;
use golem_common::schema::{
    BinaryValuePayload, FieldSource, InputSchema, SchemaGraph, SchemaType, SchemaValue,
    TextValuePayload,
};
use golem_schema::schema::render::from_untrusted_json_value;
use std::collections::{BTreeMap, HashMap};

/// Interpret the retained HTTP extraction contract using the selected method's schema.
/// Stream endpoints are supplied by execution preparation, not by the route definition.
/// The resolver must prepare unpublished candidates only. The caller owns their cleanup on
/// any decoding or resolver error; it must not durably register or publish them here.
pub fn decode_http_method_input(
    input: &HttpMethodInput,
    graph: &SchemaGraph,
    schema: &InputSchema,
    mut stream: impl FnMut(&str, &SchemaType) -> Result<SchemaValueStream, String>,
) -> Result<SchemaValue, String> {
    let json = match input.body() {
        HttpMethodBody::Json(value) => {
            let value = parse_strict_json(value.as_str().as_bytes())
                .map_err(|error| format!("Invalid HTTP JSON body: {error}"))?;
            let object = value
                .as_object()
                .ok_or("HTTP method input body must be a JSON object")?;
            for name in object.keys() {
                if !input.arguments().iter().any(|argument| {
                    matches!(&argument.source, HttpMethodArgumentSource::JsonProperty { property_name } if property_name == name)
                }) {
                    return Err(format!("HTTP JSON body property '{name}' has no argument binding"));
                }
            }
            Some(value)
        }
        HttpMethodBody::Binary { .. } | HttpMethodBody::Text { .. }
            if !input.arguments().iter().any(|argument| {
                matches!(
                    argument.source,
                    HttpMethodArgumentSource::BinaryBody | HttpMethodArgumentSource::TextBody
                )
            }) =>
        {
            return Err("HTTP method input body has no argument binding".into());
        }
        _ => None,
    };
    let mut arguments: BTreeMap<_, _> = input
        .arguments()
        .iter()
        .map(|argument| (argument.argument_name.as_str(), &argument.source))
        .collect();
    let mut values = Vec::new();
    for field in schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
    {
        let invalid = |error| format!("Invalid input parameter '{}': {error}", field.name);
        let selected = graph
            .resolve_ref(&field.schema)
            .map_err(|error| invalid(error.to_string()))?;
        let source = arguments.remove(field.name.as_str());
        let value = if matches!(selected, SchemaType::Stream { .. }) {
            if source.is_some() {
                return Err(invalid(
                    "a scalar HTTP binding cannot supply a stream".into(),
                ));
            }
            SchemaValue::Stream(stream(&field.name, selected).map_err(&invalid)?)
        } else {
            let source = source.ok_or_else(|| invalid("no HTTP argument binding".into()))?;
            let value = match source {
                HttpMethodArgumentSource::Path { value, .. } => {
                    let ty = schema_type_to_path_segment_type(graph, selected, &invalid)?;
                    parse_path_segment_value(value.clone(), &ty).map_err(|error| error.to_string())
                }
                HttpMethodArgumentSource::Query {
                    parameter_name,
                    entries,
                } => {
                    let ty = schema_type_to_query_or_header_type(graph, selected, &invalid)?;
                    let entries: HashMap<_, _> = entries.clone().into_iter().collect();
                    parse_query_parameter(parameter_name, &entries, &ty)
                        .map_err(|error| error.to_string())
                }
                HttpMethodArgumentSource::Header { values, .. } => {
                    let ty = schema_type_to_query_or_header_type(graph, selected, &invalid)?;
                    if ty.is_map() {
                        return Err(invalid("map inputs cannot bind to a header".into()));
                    }
                    parse_query_or_header_value(values, &ty).map_err(|error| error.to_string())
                }
                HttpMethodArgumentSource::JsonProperty { property_name } => {
                    let value = json.as_ref().and_then(|value| value.get(property_name));
                    match (value, input.body(), selected) {
                        (Some(value), _, _) => {
                            from_untrusted_json_value(graph, &field.schema, value)
                                .map_err(|error| error.to_string())
                        }
                        (
                            None,
                            HttpMethodBody::Absent | HttpMethodBody::Json(_),
                            SchemaType::Option { .. },
                        ) => Ok(SchemaValue::Option { inner: None }),
                        _ => Err(format!("missing JSON body property '{property_name}'")),
                    }
                }
                HttpMethodArgumentSource::BinaryBody => match input.body() {
                    HttpMethodBody::Binary { value, mime_type } => {
                        wrap_unstructured_inline_for_schema(
                            graph,
                            &field.schema,
                            SchemaValue::Binary(BinaryValuePayload {
                                bytes: value.clone(),
                                mime_type: mime_type.clone(),
                            }),
                        )
                        .map_err(|error| error.to_string())
                    }
                    _ => Err("missing binary HTTP body".into()),
                },
                HttpMethodArgumentSource::TextBody => match input.body() {
                    HttpMethodBody::Text { value, language } => {
                        wrap_unstructured_inline_for_schema(
                            graph,
                            &field.schema,
                            SchemaValue::Text(TextValuePayload {
                                text: value.clone(),
                                language: language.clone(),
                            }),
                        )
                        .map_err(|error| error.to_string())
                    }
                    _ => Err("missing text HTTP body".into()),
                },
            };
            value.map_err(&invalid)?
        };
        validate_value(graph, &field.schema, &value).map_err(|errors| {
            invalid(
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        })?;
        values.push(value);
    }
    if let Some((name, _)) = arguments.first_key_value() {
        return Err(format!(
            "HTTP argument '{name}' has no selected method input field"
        ));
    }
    Ok(SchemaValue::Record { fields: values })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::{HttpMethodArgument, JsonMethodInput};
    use golem_common::schema::NamedField;
    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::schema_type::{
        BinaryRestrictions, NumericBound, NumericRestrictions,
    };
    use test_r::test;

    #[test]
    fn selected_header_map_rejects_even_when_optional_and_absent() {
        let input = HttpMethodInput::capture(
            vec![HttpMethodArgument {
                argument_name: "labels".into(),
                source: HttpMethodArgumentSource::Header {
                    header_name: "x-labels".into(),
                    values: vec![],
                },
            }],
            HttpMethodBody::Absent,
        )
        .unwrap();
        let graph = SchemaGraph::empty();
        let original = InputSchema::parameters([NamedField::user_supplied(
            "labels",
            SchemaType::option(SchemaType::string()),
        )]);
        assert!(decode_http_method_input(&input, &graph, &original, no_stream).is_ok());

        let selected = InputSchema::parameters([NamedField::user_supplied(
            "labels",
            SchemaType::option(SchemaType::map(SchemaType::string(), SchemaType::string())),
        )]);
        assert!(
            decode_http_method_input(&input, &graph, &selected, no_stream).is_err(),
            "selected method header maps are unsupported, even if no header value was sent"
        );
    }

    fn no_stream(_: &str, _: &SchemaType) -> Result<SchemaValueStream, String> {
        Err("unexpected stream input".into())
    }

    #[test]
    fn captured_http_input_uses_selected_types_and_named_field_order() {
        let input = HttpMethodInput::capture(
            vec![
                HttpMethodArgument {
                    argument_name: "label".into(),
                    source: HttpMethodArgumentSource::JsonProperty {
                        property_name: "caption".into(),
                    },
                },
                HttpMethodArgument {
                    argument_name: "count".into(),
                    source: HttpMethodArgumentSource::Query {
                        parameter_name: "n".into(),
                        entries: BTreeMap::from([("n".into(), vec!["513".into()])]),
                    },
                },
                HttpMethodArgument {
                    argument_name: "tail".into(),
                    source: HttpMethodArgumentSource::JsonProperty {
                        property_name: "suffix".into(),
                    },
                },
            ],
            HttpMethodBody::Json(
                JsonMethodInput::capture(br#"{"caption":"first","suffix":"last"}"#).unwrap(),
            ),
        )
        .unwrap();
        let old = InputSchema::parameters([
            NamedField::user_supplied("label", SchemaType::string()),
            NamedField::user_supplied("count", SchemaType::u8()),
            NamedField::user_supplied("tail", SchemaType::string()),
        ]);
        let selected = InputSchema::parameters([
            NamedField::user_supplied("tail", SchemaType::string()),
            NamedField::user_supplied("count", SchemaType::u32()),
            NamedField::user_supplied("label", SchemaType::string()),
        ]);
        let graph = SchemaGraph::empty();
        assert!(decode_http_method_input(&input, &graph, &old, no_stream).is_err());
        assert_eq!(
            decode_http_method_input(&input, &graph, &selected, no_stream).unwrap(),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("last".into()),
                    SchemaValue::U32(513),
                    SchemaValue::String("first".into())
                ]
            },
        );
    }

    #[test]
    fn selected_http_bindings_reject_removed_missing_and_stream_changed_fields() {
        let input = HttpMethodInput::capture(
            vec![HttpMethodArgument {
                argument_name: "amount".into(),
                source: HttpMethodArgumentSource::Path {
                    path_segment_index: 2,
                    value: "7".into(),
                },
            }],
            HttpMethodBody::Absent,
        )
        .unwrap();
        let graph = SchemaGraph::empty();
        for schema in [
            InputSchema::parameters([]),
            InputSchema::parameters([NamedField::user_supplied("other", SchemaType::u32())]),
            InputSchema::parameters([NamedField::user_supplied(
                "amount",
                SchemaType::stream(Some(SchemaType::u32())),
            )]),
        ] {
            assert!(decode_http_method_input(&input, &graph, &schema, no_stream).is_err());
        }
        let bounded = InputSchema::parameters([NamedField::user_supplied(
            "amount",
            SchemaType::U32 {
                restrictions: Some(NumericRestrictions {
                    max: Some(NumericBound::Unsigned(6)),
                    ..Default::default()
                }),
                metadata: Default::default(),
            },
        )]);
        assert!(decode_http_method_input(&input, &graph, &bounded, no_stream).is_err());
    }

    #[test]
    fn selected_http_streams_and_absent_json_options_do_not_need_route_types() {
        let input = HttpMethodInput::capture(
            vec![HttpMethodArgument {
                argument_name: "label".into(),
                source: HttpMethodArgumentSource::JsonProperty {
                    property_name: "caption".into(),
                },
            }],
            HttpMethodBody::Json(JsonMethodInput::capture(b"{}").unwrap()),
        )
        .unwrap();
        let schema = InputSchema::parameters([
            NamedField::user_supplied("updates", SchemaType::stream(Some(SchemaType::u32()))),
            NamedField::user_supplied("label", SchemaType::option(SchemaType::string())),
        ]);
        let value =
            decode_http_method_input(&input, &SchemaGraph::empty(), &schema, |name, schema| {
                assert_eq!(name, "updates");
                assert_eq!(schema, &SchemaType::stream(Some(SchemaType::u32())));
                Ok(SchemaValueStream::from_host_endpoint(42u64))
            })
            .unwrap();
        let SchemaValue::Record { mut fields } = value else {
            panic!("expected record")
        };
        assert_eq!(fields.pop(), Some(SchemaValue::Option { inner: None }));
        let SchemaValue::Stream(stream) = fields.pop().unwrap() else {
            panic!("expected stream")
        };
        assert_eq!(stream.take_host_endpoint::<u64>().unwrap(), 42);
    }

    #[test]
    fn selected_http_binary_schema_enforces_mime_restrictions() {
        let input = HttpMethodInput::capture(
            vec![HttpMethodArgument {
                argument_name: "data".into(),
                source: HttpMethodArgumentSource::BinaryBody,
            }],
            HttpMethodBody::Binary {
                value: vec![1, 2, 3],
                mime_type: Some("image/png".into()),
            },
        )
        .unwrap();
        let selected = InputSchema::parameters([NamedField::user_supplied(
            "data",
            SchemaType::binary(BinaryRestrictions::default()),
        )]);
        assert_eq!(
            decode_http_method_input(&input, &SchemaGraph::empty(), &selected, no_stream).unwrap(),
            SchemaValue::Record {
                fields: vec![SchemaValue::Binary(BinaryValuePayload {
                    bytes: vec![1, 2, 3],
                    mime_type: Some("image/png".into())
                })],
            }
        );
        let restricted = InputSchema::parameters([NamedField::user_supplied(
            "data",
            SchemaType::binary(BinaryRestrictions {
                mime_types: Some(vec!["application/pdf".into()]),
                ..Default::default()
            }),
        )]);
        assert!(
            decode_http_method_input(&input, &SchemaGraph::empty(), &restricted, no_stream)
                .is_err()
        );
    }

    #[test]
    fn selected_http_query_collections_resolve_named_types() {
        let number = SchemaType::Ref {
            id: TypeId::new("number"),
            metadata: Default::default(),
        };
        let mut graph = SchemaGraph::empty();
        graph.defs.push(SchemaTypeDef {
            id: TypeId::new("number"),
            name: None,
            body: SchemaType::u32(),
        });
        let input = HttpMethodInput::capture(
            vec![HttpMethodArgument {
                argument_name: "counts".into(),
                source: HttpMethodArgumentSource::Query {
                    parameter_name: "n".into(),
                    entries: BTreeMap::from([
                        ("n[2]".into(), vec!["7".into()]),
                        ("n[10]".into(), vec!["13".into()]),
                    ]),
                },
            }],
            HttpMethodBody::Absent,
        )
        .unwrap();
        let schema = InputSchema::parameters([NamedField::user_supplied(
            "counts",
            SchemaType::option(SchemaType::map(number.clone(), number)),
        )]);
        assert_eq!(
            decode_http_method_input(&input, &graph, &schema, no_stream).unwrap(),
            SchemaValue::Record {
                fields: vec![SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Map {
                        entries: vec![
                            (SchemaValue::U32(10), SchemaValue::U32(13)),
                            (SchemaValue::U32(2), SchemaValue::U32(7)),
                        ]
                    })),
                }]
            },
        );
    }
}
