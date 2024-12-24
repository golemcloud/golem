// rib_to_openapi.rs
// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_wasm_ast::analysis::AnalysedType;
use rib::RibInputTypeInfo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Represents an OpenAPI schema object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OpenApiSchema {
    Boolean,
    Integer {
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    String {
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        enum_values: Option<Vec<String>>,
    },
    Array {
        items: Box<OpenApiSchema>,
    },
    Object {
        properties: BTreeMap<String, OpenApiSchema>,
        #[serde(skip_serializing_if = "Option::is_none")]
        required: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        additional_properties: Option<Box<OpenApiSchema>>,
    },
    Enum {
        values: Vec<String>,
    },
    Ref {
        reference: String,
    },
}

pub struct RibToOpenApi;

impl RibToOpenApi {
    /// Converts a RibInputTypeInfo into an OpenAPI schema.
    pub fn convert(input_type: &RibInputTypeInfo) -> OpenApiSchema {
        let mut properties = BTreeMap::new();
        let mut required = Vec::new();

        for (name, analysed_type) in &input_type.types {
            if let Some(schema) = RibToOpenApi::convert_type(analysed_type) {
                properties.insert(name.clone(), schema);
                required.push(name.clone());
            }
        }

        OpenApiSchema::Object {
            properties,
            required: if required.is_empty() {
                None
            } else {
                Some(required)
            },
            additional_properties: None,
        }
    }

    // Security configuration for OpenAPI
    pub fn generate_security_scheme() -> OpenApiSchema {
        OpenApiSchema::Object {
            properties: BTreeMap::from([
                ("bearerAuth".to_string(), OpenApiSchema::Object {
                    properties: BTreeMap::new(),
                    required: Some(vec![]),
                    additional_properties: None,
                }),
            ]),
            required: None,
            additional_properties: None,
        }
    }

    /// Converts an AnalysedType into an OpenAPI schema.
    pub fn convert_type(analysed_type: &AnalysedType) -> Option<OpenApiSchema> {
        match analysed_type {
            AnalysedType::Bool(_) => Some(OpenApiSchema::Boolean),

            AnalysedType::U8(_) | AnalysedType::U32(_) | AnalysedType::U64(_) => {
                Some(OpenApiSchema::Integer { format: None })
            }

            AnalysedType::F32(_) | AnalysedType::F64(_) => Some(OpenApiSchema::Number {
                format: Some("float".to_string()),
            }),

            AnalysedType::Str(_) => Some(OpenApiSchema::String {
                format: None,
                enum_values: None,
            }),

            AnalysedType::List(inner_type) => {
                let schema = RibToOpenApi::convert_type(&inner_type.inner)?;
                Some(OpenApiSchema::Array {
                    items: Box::new(schema),
                })
            }

            AnalysedType::Record(record) => {
                let mut properties = BTreeMap::new();
                let mut required = Vec::new();

                for field in &record.fields {
                    if let Some(field_schema) = RibToOpenApi::convert_type(&field.typ) {
                        properties.insert(field.name.clone(), field_schema);
                        required.push(field.name.clone());
                    }
                }

                Some(OpenApiSchema::Object {
                    properties,
                    required: if required.is_empty() {
                        None
                    } else {
                        Some(required)
                    },
                    additional_properties: None,
                })
            }

            AnalysedType::Enum(enum_type) => Some(OpenApiSchema::Enum {
                values: enum_type.cases.clone(),
            }),

            AnalysedType::Variant(variant_type) => {
                let mut one_of = Vec::new();

                for case in &variant_type.cases {
                    if let Some(typ) = &case.typ {
                        if let Some(case_schema) = RibToOpenApi::convert_type(typ) {
                            let object_schema = OpenApiSchema::Object {
                                properties: BTreeMap::from([(case.name.clone(), case_schema)]),
                                required: Some(vec![case.name.clone()]),
                                additional_properties: None,
                            };
                            one_of.push(object_schema);
                        }
                    }
                }

                if !one_of.is_empty() {
                    Some(OpenApiSchema::Object {
                        properties: BTreeMap::new(),
                        required: None,
                        additional_properties: Some(Box::new(OpenApiSchema::Array {
                            items: Box::new(OpenApiSchema::String {
                                format: None,
                                enum_values: None,
                            }),
                        })),
                    })
                } else {
                    None
                }
            }

            AnalysedType::Option(option_type) => {
                let inner_schema = RibToOpenApi::convert_type(&option_type.inner)?;
                Some(OpenApiSchema::Object {
                    properties: BTreeMap::from([("value".to_string(), inner_schema)]),
                    required: None,
                    additional_properties: None,
                })
            }

            AnalysedType::Result(result_type) => {
                let mut properties = BTreeMap::new();

                if let Some(ok_type) = &result_type.ok {
                    if let Some(ok_schema) = RibToOpenApi::convert_type(ok_type) {
                        properties.insert("ok".to_string(), ok_schema);
                    }
                }

                if let Some(err_type) = &result_type.err {
                    if let Some(err_schema) = RibToOpenApi::convert_type(err_type) {
                        properties.insert("err".to_string(), err_schema);
                    }
                }

                Some(OpenApiSchema::Object {
                    properties,
                    required: None,
                    additional_properties: None,
                })
            }

            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm_ast::analysis::{model, AnalysedType};

    #[test]
    fn test_generate_security_scheme() {
        let security = RibToOpenApi::generate_security_scheme();
        if let OpenApiSchema::Object { properties, .. } = security {
            assert!(properties.contains_key("bearerAuth"));
        } else {
            panic!("Security scheme not generated correctly");
        }
    }

    #[test]
    fn test_convert_simple_types() {
        assert!(matches!(
            RibToOpenApi::convert_type(&AnalysedType::Bool(model::TypeBool {})),
            Some(OpenApiSchema::Boolean)
        ));
        assert!(matches!(
            RibToOpenApi::convert_type(&AnalysedType::Str(model::TypeStr {})),
            Some(OpenApiSchema::String { .. })
        ));
    }

    #[test]
    fn test_convert_record_type() {
        let record = AnalysedType::Record(model::TypeRecord {
            fields: vec![
                model::NameTypePair {
                    name: "field1".to_string(),
                    typ: AnalysedType::Bool(model::TypeBool {}),
                },
                model::NameTypePair {
                    name: "field2".to_string(),
                    typ: AnalysedType::U32(model::TypeU32 {}),
                },
            ],
        });

        let schema = RibToOpenApi::convert_type(&record);
        if let Some(OpenApiSchema::Object {
            properties,
            required,
            ..
        }) = schema
        {
            assert_eq!(properties.len(), 2);
            assert!(properties.contains_key("field1"));
            assert!(properties.contains_key("field2"));
            assert_eq!(
                required.unwrap(),
                vec!["field1".to_string(), "field2".to_string()]
            );
        } else {
            panic!("Record type not converted correctly");
        }
    }

    /// Tests for enum types.
    #[test]
    fn test_convert_enum_type() {
        let enum_type = AnalysedType::Enum(model::TypeEnum {
            cases: vec!["case1".to_string(), "case2".to_string()],
        });

        let schema = RibToOpenApi::convert_type(&enum_type);

        if let Some(OpenApiSchema::Enum { values }) = schema {
            assert_eq!(
                values,
                vec!["case1".to_string(), "case2".to_string()],
                "Enum values do not match expected cases"
            );
        } else {
            panic!("Enum type not converted correctly");
        }
    }

    /// Tests for nested structures.
    #[test]
    fn test_convert_nested_structures() {
        let nested_record = AnalysedType::Record(model::TypeRecord {
            fields: vec![model::NameTypePair {
                name: "nested".to_string(),
                typ: AnalysedType::Record(model::TypeRecord {
                    fields: vec![model::NameTypePair {
                        name: "inner".to_string(),
                        typ: AnalysedType::U32(model::TypeU32 {}),
                    }],
                }),
            }],
        });

        let schema = RibToOpenApi::convert_type(&nested_record);

        if let Some(OpenApiSchema::Object { properties, .. }) = schema {
            assert!(
                properties.contains_key("nested"),
                "Nested structure does not contain expected key 'nested'"
            );
            if let Some(nested_schema) = properties.get("nested") {
                if let OpenApiSchema::Object {
                    properties: inner_properties,
                    ..
                } = nested_schema
                {
                    assert!(
                        inner_properties.contains_key("inner"),
                        "Inner nested structure does not contain expected key 'inner'"
                    );
                } else {
                    panic!("Expected 'nested' to be an Object schema");
                }
            } else {
                panic!("Schema for 'nested' not found");
            }
        } else {
            panic!("Nested record type not converted correctly");
        }
    }

    #[test]
    fn test_convert_optional_type() {
        // Test for an Optional type (Option)
        let optional_type = AnalysedType::Option(model::TypeOption {
            inner: Box::new(AnalysedType::Str(model::TypeStr {})),
        });

        let schema = RibToOpenApi::convert_type(&optional_type);

        if let Some(OpenApiSchema::Object {
            properties,
            required,
            ..
        }) = schema
        {
            assert!(
                properties.contains_key("value"),
                "Optional type schema should contain a key 'value'"
            );
            assert!(
                required.is_none(),
                "Optional type schema should not have required properties"
            );
        } else {
            panic!("Optional type not converted correctly");
        }
    }

    #[test]
    fn test_convert_result_type() {
        // Test for a Result type with Ok and Err
        let result_type = AnalysedType::Result(model::TypeResult {
            ok: Some(Box::new(AnalysedType::U8(model::TypeU8 {}))),
            err: Some(Box::new(AnalysedType::Str(model::TypeStr {}))),
        });

        let schema = RibToOpenApi::convert_type(&result_type);

        if let Some(OpenApiSchema::Object {
            properties,
            required,
            ..
        }) = schema
        {
            // Check that the properties contain both "ok" and "err"
            assert!(
                properties.contains_key("ok"),
                "Result type schema should contain key 'ok'"
            );
            assert!(
                properties.contains_key("err"),
                "Result type schema should contain key 'err'"
            );
            // Check that required fields are empty
            assert!(
                required.is_none() || required.as_ref().unwrap().is_empty(),
                "Result type schema should not have required properties"
            );
        } else {
            panic!("Result type not converted correctly");
        }
    }

    #[test]
    fn test_convert_empty_list_type() {
        // Test for a List type with a single inner type (String)
        let empty_list_type = AnalysedType::List(model::TypeList {
            inner: Box::new(AnalysedType::Str(model::TypeStr {})),
        });

        let schema = RibToOpenApi::convert_type(&empty_list_type);

        if let Some(OpenApiSchema::Array { items }) = schema {
            // Check that the inner schema matches the expected type (String)
            assert!(
                matches!(*items, OpenApiSchema::String { .. }),
                "List inner schema should be of type 'String'"
            );
        } else {
            panic!("List type not converted correctly");
        }
    }

    #[test]
    fn test_convert_nested_list_type() {
        let nested_list_type = AnalysedType::List(model::TypeList {
            inner: Box::new(AnalysedType::List(model::TypeList {
                inner: Box::new(AnalysedType::U32(model::TypeU32 {})),
            })),
        });

        let schema = RibToOpenApi::convert_type(&nested_list_type);
        if let Some(OpenApiSchema::Array(inner_schema)) = schema {
            if let OpenApiSchema::Array(nested_inner_schema) = *inner_schema {
                assert!(matches!(
                    *nested_inner_schema,
                    OpenApiSchema::Integer { .. }
                ));
            } else {
                panic!("Nested list type not converted correctly");
            }
        } else {
            panic!("List type not converted correctly");
        }
    }

    #[test]
    fn test_convert_record_with_empty_fields() {
        // Test for a Record with no fields
        let empty_record = AnalysedType::Record(model::TypeRecord { fields: vec![] });

        let schema = RibToOpenApi::convert_type(&empty_record);

        if let Some(OpenApiSchema::Object {
            properties,
            required,
            ..
        }) = schema
        {
            assert!(
                properties.is_empty(),
                "Properties should be empty for a record with no fields"
            );
            assert!(
                required.is_none(),
                "Required should be None for a record with no fields"
            );
        } else {
            panic!("Empty record type not converted correctly");
        }
    }

    #[test]
    fn test_convert_enum_with_one_case() {
        // Test for an Enum with a single case
        let single_case_enum = AnalysedType::Enum(model::TypeEnum {
            cases: vec!["only_case".to_string()],
        });

        let schema = RibToOpenApi::convert_type(&single_case_enum);

        if let Some(OpenApiSchema::Enum { values }) = schema {
            assert_eq!(
                values,
                vec!["only_case".to_string()],
                "Enum values do not match the expected single case"
            );
        } else {
            panic!("Enum with one case not converted correctly");
        }
    }

    #[test]
    fn test_unsupported_type() {
        // Test for an unsupported type
        let unsupported_type = AnalysedType::Tuple(model::TypeTuple { items: vec![] });

        let schema = RibToOpenApi::convert_type(&unsupported_type);

        assert!(
            schema.is_none(),
            "Unsupported type should return None, but got some schema"
        );
    }

    /// Test for complex nested types
    #[test]
    fn test_complex_nested_type() {
        let nested_record = AnalysedType::Record(model::TypeRecord {
            fields: vec![model::NameTypePair {
                name: "items".to_string(),
                typ: AnalysedType::List(model::TypeList {
                    inner: Box::new(AnalysedType::Record(model::TypeRecord {
                        fields: vec![
                            model::NameTypePair {
                                name: "id".to_string(),
                                typ: AnalysedType::U64(model::TypeU64),
                            },
                            model::NameTypePair {
                                name: "name".to_string(),
                                typ: AnalysedType::Option(model::TypeOption {
                                    inner: Box::new(AnalysedType::Str(model::TypeStr)),
                                }),
                            },
                        ],
                    })),
                }),
            }],
        });

        let schema = RibToOpenApi::convert_type(&nested_record).unwrap();

        if let OpenApiSchema::Object { properties, .. } = schema {
            assert!(properties.contains_key("items"));

            if let OpenApiSchema::Array { items } = properties["items"].clone() {
                if let OpenApiSchema::Object { properties: nested_properties, .. } = *items {
                    assert!(nested_properties.contains_key("id"));
                    assert!(nested_properties.contains_key("name"));
                } else {
                    panic!("Expected nested object schema");
                }
            } else {
                panic!("Expected array schema");
            }
        } else {
            panic!("Expected object schema");
        }
    }

    /// Test for primitive types
    #[test]
    fn test_primitive_types() {
        let bool_type = AnalysedType::Bool(model::TypeBool);
        let schema = RibToOpenApi::convert_type(&bool_type).unwrap();
        assert!(matches!(schema, OpenApiSchema::Boolean));

        let int_type = AnalysedType::U32(model::TypeU32);
        let schema = RibToOpenApi::convert_type(&int_type).unwrap();
        assert!(matches!(schema, OpenApiSchema::Integer { .. }));

        let float_type = AnalysedType::F64(model::TypeF64);
        let schema = RibToOpenApi::convert_type(&float_type).unwrap();
        assert!(matches!(schema, OpenApiSchema::Number { .. }));

        let string_type = AnalysedType::Str(model::TypeStr);
        let schema = RibToOpenApi::convert_type(&string_type).unwrap();
        assert!(matches!(schema, OpenApiSchema::String { .. }));
    }

    /// Test for lists, options and enums
    #[test]
    fn test_container_types() {
        let list_type = AnalysedType::List(model::TypeList {
            inner: Box::new(AnalysedType::Str(model::TypeStr)),
        });
        let schema = RibToOpenApi::convert_type(&list_type).unwrap();
        assert!(matches!(schema, OpenApiSchema::Array { .. }));

        let option_type = AnalysedType::Option(model::TypeOption {
            inner: Box::new(AnalysedType::Str(model::TypeStr)),
        });
        let schema = RibToOpenApi::convert_type(&option_type).unwrap();
        if let OpenApiSchema::Object { properties, .. } = schema {
            assert!(properties.contains_key("value"));
        } else {
            panic!("Expected object schema for option type");
        }

        let enum_type = AnalysedType::Enum(model::TypeEnum {
            cases: vec!["case1".to_string(), "case2".to_string()],
        });
        let schema = RibToOpenApi::convert_type(&enum_type).unwrap();
        if let OpenApiSchema::Enum { values } = schema {
            assert_eq!(values, vec!["case1".to_string(), "case2".to_string()]);
        } else {
            panic!("Expected enum schema");
        }
    }

}
