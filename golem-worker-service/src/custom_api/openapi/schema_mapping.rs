// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use openapiv3::{
    AdditionalProperties, ArrayType, BooleanType, IntegerFormat, IntegerType, NumberFormat,
    NumberType, ObjectType, ReferenceOr, Schema, SchemaData, SchemaKind, StringType, Type,
    VariantOrUnknownOrEmpty,
};

pub fn create_schema_from_analysed_type(analysed_type: &AnalysedType) -> Schema {
    use golem_wasm::analysis::AnalysedType;

    match analysed_type {
        AnalysedType::Bool(_) => Schema {
            schema_data: Default::default(),
            schema_kind: SchemaKind::Type(Type::Boolean(BooleanType::default())),
        },
        AnalysedType::U8(_) => create_integer_schema(IntegerFormat::Int32, Some(0), Some(255)),
        AnalysedType::U16(_) => create_integer_schema(IntegerFormat::Int32, Some(0), Some(65535)),
        AnalysedType::U32(_) => create_integer_schema(IntegerFormat::Int32, Some(0), None),
        AnalysedType::U64(_) => create_integer_schema(IntegerFormat::Int64, Some(0), None),
        AnalysedType::S8(_) => create_integer_schema(IntegerFormat::Int32, Some(-128), Some(127)),
        AnalysedType::S16(_) => {
            create_integer_schema(IntegerFormat::Int32, Some(-32768), Some(32767))
        }
        AnalysedType::S32(_) => create_integer_schema(IntegerFormat::Int32, None, None),
        AnalysedType::S64(_) => create_integer_schema(IntegerFormat::Int64, None, None),

        AnalysedType::F32(_) => {
            let sd = SchemaData::default();
            Schema {
                schema_data: sd,
                schema_kind: SchemaKind::Type(Type::Number(NumberType {
                    format: VariantOrUnknownOrEmpty::Item(NumberFormat::Float),
                    multiple_of: None,
                    exclusive_minimum: false,
                    exclusive_maximum: false,
                    minimum: None,
                    maximum: None,
                    enumeration: vec![],
                })),
            }
        }
        AnalysedType::F64(_) => {
            let sd = SchemaData::default();
            Schema {
                schema_data: sd,
                schema_kind: SchemaKind::Type(Type::Number(NumberType {
                    format: VariantOrUnknownOrEmpty::Item(NumberFormat::Double),
                    multiple_of: None,
                    exclusive_minimum: false,
                    exclusive_maximum: false,
                    minimum: None,
                    maximum: None,
                    enumeration: vec![],
                })),
            }
        }
        AnalysedType::Str(_) => Schema {
            schema_data: Default::default(),
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: None,
                enumeration: vec![],
                min_length: None,
                max_length: None,
            })),
        },
        AnalysedType::List(type_list) => {
            let items =
                ReferenceOr::Item(Box::new(create_schema_from_analysed_type(&type_list.inner)));
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(items),
                    min_items: None,
                    max_items: None,
                    unique_items: false,
                })),
            }
        }
        AnalysedType::Tuple(type_tuple) => {
            let min_items = Some(type_tuple.items.len());
            let max_items = Some(type_tuple.items.len());

            let items = ReferenceOr::Item(Box::new(Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(Default::default())),
            }));
            let array_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(items),
                    min_items,
                    max_items,
                    unique_items: false,
                })),
            };

            let schema_data = SchemaData {
                description: Some("Tuple type".to_string()),
                ..Default::default()
            };
            Schema {
                schema_data,
                schema_kind: array_schema.schema_kind,
            }
        }
        AnalysedType::Record(type_record) => {
            let mut properties = IndexMap::new();
            let mut required = Vec::new();
            for field in &type_record.fields {
                let field_schema = create_schema_from_analysed_type(&field.typ);
                let is_nullable = field_schema.schema_data.nullable;
                properties.insert(
                    field.name.clone(),
                    ReferenceOr::Item(Box::new(field_schema)),
                );
                if !is_nullable {
                    required.push(field.name.clone());
                }
            }
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties,
                    required,
                    additional_properties: None,
                    min_properties: None,
                    max_properties: None,
                })),
            }
        }
        AnalysedType::Variant(type_variant) => {
            let mut one_of = Vec::new();
            for case in &type_variant.cases {
                let case_name = &case.name;
                if let Some(case_type) = &case.typ {
                    let case_schema = create_schema_from_analysed_type(case_type);
                    let mut properties = IndexMap::new();
                    properties.insert(case_name.clone(), ReferenceOr::Item(Box::new(case_schema)));
                    let required = vec![case_name.clone()];
                    let schema = Schema {
                        schema_data: Default::default(),
                        schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                            properties,
                            required,
                            additional_properties: None,
                            min_properties: None,
                            max_properties: None,
                        })),
                    };
                    one_of.push(ReferenceOr::Item(schema));
                } else {
                    let schema = Schema {
                        schema_data: Default::default(),
                        schema_kind: SchemaKind::Type(Type::String(StringType {
                            format: VariantOrUnknownOrEmpty::Empty,
                            pattern: None,
                            enumeration: vec![Some(case_name.clone())],
                            min_length: None,
                            max_length: None,
                        })),
                    };

                    one_of.push(ReferenceOr::Item(schema));
                }
            }
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::OneOf { one_of },
            }
        }
        AnalysedType::Enum(type_enum) => {
            let enum_values: Vec<Option<String>> =
                type_enum.cases.iter().map(|c| Some(c.clone())).collect();
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: enum_values,
                    min_length: None,
                    max_length: None,
                })),
            }
        }
        AnalysedType::Option(type_option) => {
            let mut schema = create_schema_from_analysed_type(&type_option.inner);
            schema.schema_data.nullable = true;
            schema
        }
        AnalysedType::Result(type_result) => {
            let ok_type = match &type_result.ok {
                Some(b) => &**b,
                None => &AnalysedType::Str(golem_wasm::analysis::TypeStr {}),
            };
            let err_type = match &type_result.err {
                Some(b) => &**b,
                None => &AnalysedType::Str(golem_wasm::analysis::TypeStr {}),
            };
            let ok_schema = create_schema_from_analysed_type(ok_type);
            let err_schema = create_schema_from_analysed_type(err_type);

            let mut ok_properties = IndexMap::new();
            ok_properties.insert("ok".to_string(), ReferenceOr::Item(Box::new(ok_schema)));
            let ok_required = vec!["ok".to_string()];
            let ok_object_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties: ok_properties,
                    required: ok_required,
                    additional_properties: Some(AdditionalProperties::Any(false)),
                    min_properties: None,
                    max_properties: None,
                })),
            };

            let mut err_properties = IndexMap::new();
            err_properties.insert("err".to_string(), ReferenceOr::Item(Box::new(err_schema)));
            let err_required = vec!["err".to_string()];
            let err_object_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties: err_properties,
                    required: err_required,
                    additional_properties: Some(AdditionalProperties::Any(false)),
                    min_properties: None,
                    max_properties: None,
                })),
            };

            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::OneOf {
                    one_of: vec![
                        ReferenceOr::Item(ok_object_schema),
                        ReferenceOr::Item(err_object_schema),
                    ],
                },
            }
        }
        AnalysedType::Flags(type_flags) => {
            let enum_values: Vec<Option<String>> =
                type_flags.names.iter().map(|n| Some(n.clone())).collect();
            let items_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: enum_values,
                    min_length: None,
                    max_length: None,
                })),
            };
            Schema {
                schema_data: SchemaData {
                    description: Some("Flags type - array of flag names".to_string()),
                    ..Default::default()
                },
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(ReferenceOr::Item(Box::new(items_schema))),
                    min_items: Some(0),
                    max_items: Some(type_flags.names.len()),
                    unique_items: true,
                })),
            }
        }
        AnalysedType::Chr(_) => Schema {
            schema_data: SchemaData {
                description: Some("Unicode character".to_string()),
                ..Default::default()
            },
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: Some("^.{1}$".to_string()),
                enumeration: vec![],
                min_length: Some(1),
                max_length: Some(1),
            })),
        },
        AnalysedType::Handle(_) => Schema {
            schema_data: SchemaData {
                description: Some("Opaque handle identifier".to_string()),
                ..Default::default()
            },
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: None,
                enumeration: vec![],
                min_length: None,
                max_length: None,
            })),
        },
    }
}

fn create_integer_schema(format: IntegerFormat, min: Option<i64>, max: Option<i64>) -> Schema {
    let schema_data = SchemaData::default();
    Schema {
        schema_data,
        schema_kind: SchemaKind::Type(Type::Integer(IntegerType {
            format: VariantOrUnknownOrEmpty::Item(format),
            minimum: min,
            maximum: max,
            multiple_of: None,
            exclusive_minimum: false,
            exclusive_maximum: false,
            enumeration: vec![],
        })),
    }
}
