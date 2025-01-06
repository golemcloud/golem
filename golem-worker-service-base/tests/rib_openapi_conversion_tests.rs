#[cfg(test)]
mod tests {
    use golem_wasm_ast::analysis::{
        AnalysedType,
        TypeBool,
        TypeChr,
        TypeEnum,
        TypeF32,
        TypeF64,
        TypeList,
        TypeRecord,
        TypeResult,
        TypeS16,
        TypeS32,
        TypeS64,
        TypeS8,
        TypeStr,
        TypeU16,
        TypeU32,
        TypeU64,
        TypeU8,
        TypeVariant,
        NameTypePair,
        NameOptionTypePair,
    };
    use serde_json::Value;
    use poem_openapi::registry::{MetaSchemaRef, Registry};
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;

    // Helper function to verify schema type
    fn assert_schema_type(schema: &MetaSchemaRef, expected_type: &str) {
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, expected_type);
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema, got reference"),
        }
    }

    // Helper function to find property in schema
    fn find_property<'a>(properties: &'a [(&'static str, MetaSchemaRef)], key: &str) -> Option<&'a MetaSchemaRef> {
        properties.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    // Helper function to check if property exists
    fn has_property(properties: &[(&'static str, MetaSchemaRef)], key: &str) -> bool {
        properties.iter().any(|(k, _)| *k == key)
    }

    // Helper function to verify schema format
    fn assert_schema_format(schema: &MetaSchemaRef, expected_format: &str) {
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.format.as_deref(), Some(expected_format));
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema, got reference"),
        }
    }

    #[test]
    fn test_primitive_types() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        // Boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type, &mut registry).unwrap();
        assert_schema_type(&schema, "boolean");

        // Integer types with proper formats
        let u8_type = AnalysedType::U8(TypeU8);
        let schema = converter.convert_type(&u8_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let u16_type = AnalysedType::U16(TypeU16);
        let schema = converter.convert_type(&u16_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let u32_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&u32_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let u64_type = AnalysedType::U64(TypeU64);
        let schema = converter.convert_type(&u64_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int64");

        let s8_type = AnalysedType::S8(TypeS8);
        let schema = converter.convert_type(&s8_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let s16_type = AnalysedType::S16(TypeS16);
        let schema = converter.convert_type(&s16_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let s32_type = AnalysedType::S32(TypeS32);
        let schema = converter.convert_type(&s32_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int32");

        let s64_type = AnalysedType::S64(TypeS64);
        let schema = converter.convert_type(&s64_type, &mut registry).unwrap();
        assert_schema_type(&schema, "integer");
        assert_schema_format(&schema, "int64");

        // Float types with proper formats
        let f32_type = AnalysedType::F32(TypeF32);
        let schema = converter.convert_type(&f32_type, &mut registry).unwrap();
        assert_schema_type(&schema, "number");
        assert_schema_format(&schema, "float");

        let f64_type = AnalysedType::F64(TypeF64);
        let schema = converter.convert_type(&f64_type, &mut registry).unwrap();
        assert_schema_type(&schema, "number");
        assert_schema_format(&schema, "double");

        // String and Char
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type, &mut registry).unwrap();
        assert_schema_type(&schema, "string");

        let char_type = AnalysedType::Chr(TypeChr);
        let schema = converter.convert_type(&char_type, &mut registry).unwrap();
        assert_schema_type(&schema, "string");
        match &schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.min_length, Some(1));
                assert_eq!(schema.max_length, Some(1));
            },
            _ => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_list_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        let list_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        });

        let schema = converter.convert_type(&list_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "array");
                assert!(schema.items.is_some());
                if let Some(items) = &schema.items {
                    match &**items {
                        MetaSchemaRef::Inline(items_schema) => {
                            assert_eq!(items_schema.ty, "string");
                        },
                        MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
                    }
                }
                // Verify array constraints
                assert_eq!(schema.min_items, Some(0));
                assert_eq!(schema.unique_items, Some(false));
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_record_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "field1".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "field2".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "email".to_string(), // Special field name to test format
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        });

        let schema = converter.convert_type(&record_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "object");
                assert!(has_property(&schema.properties, "field1"));
                assert!(has_property(&schema.properties, "field2"));
                assert!(has_property(&schema.properties, "email"));
                assert_eq!(schema.required.len(), 3);
                assert!(schema.required.contains(&"field1"));
                assert!(schema.required.contains(&"field2"));
                assert!(schema.required.contains(&"email"));

                let field1_schema = find_property(&schema.properties, "field1").unwrap();
                assert_schema_type(field1_schema, "integer");

                let field2_schema = find_property(&schema.properties, "field2").unwrap();
                assert_schema_type(field2_schema, "string");

                let email_schema = find_property(&schema.properties, "email").unwrap();
                assert_schema_type(email_schema, "string");
                match email_schema {
                    MetaSchemaRef::Inline(schema) => {
                        assert_eq!(schema.format.as_deref(), Some("email"));
                    },
                    _ => panic!("Expected inline schema"),
                }

                // Verify additionalProperties is false
                match schema.additional_properties.as_deref() {
                    Some(MetaSchemaRef::Inline(additional_props)) => {
                        assert_eq!(additional_props.ty, "boolean");
                    },
                    _ => panic!("Expected additional_properties to be false"),
                }
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_variant_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        let variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Case1".to_string(),
                    typ: Some(AnalysedType::U32(TypeU32)),
                },
                NameOptionTypePair {
                    name: "Case2".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
                NameOptionTypePair {
                    name: "Case3".to_string(),
                    typ: None,
                },
            ],
        });

        let schema = converter.convert_type(&variant_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "object");
                assert!(schema.required.contains(&"type"));

                // Verify type discriminator
                let type_schema = find_property(&schema.properties, "type").unwrap();
                match type_schema {
                    MetaSchemaRef::Inline(schema) => {
                        assert_eq!(schema.ty, "string");
                        assert_eq!(schema.enum_items.len(), 3);
                        assert!(schema.enum_items.contains(&Value::String("Case1".to_string())));
                        assert!(schema.enum_items.contains(&Value::String("Case2".to_string())));
                        assert!(schema.enum_items.contains(&Value::String("Case3".to_string())));
                    },
                    _ => panic!("Expected inline schema"),
                }

                // Verify value property for cases with types
                if let Some(value_schema) = find_property(&schema.properties, "value") {
                    match value_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "object");
                            assert!(!schema.one_of.is_empty());
                            assert_eq!(schema.one_of.len(), 2); // Only Case1 and Case2 have types
                        },
                        _ => panic!("Expected inline schema"),
                    }
                }
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_result_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        let result_type = AnalysedType::Result(TypeResult {
            ok: Some(Box::new(AnalysedType::U32(TypeU32))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        });

        let schema = converter.convert_type(&result_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "object");
                assert!(schema.required.contains(&"type"));

                // Verify type discriminator
                let type_schema = find_property(&schema.properties, "type").unwrap();
                match type_schema {
                    MetaSchemaRef::Inline(schema) => {
                        assert_eq!(schema.ty, "string");
                        assert_eq!(schema.enum_items.len(), 2);
                        assert!(schema.enum_items.contains(&Value::String("ok".to_string())));
                        assert!(schema.enum_items.contains(&Value::String("error".to_string())));
                    },
                    _ => panic!("Expected inline schema"),
                }

                // Verify value property
                if let Some(value_schema) = find_property(&schema.properties, "value") {
                    match value_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "object");
                            assert!(!schema.one_of.is_empty());
                            assert_eq!(schema.one_of.len(), 2);
                        },
                        _ => panic!("Expected inline schema"),
                    }
                }
            },
            _ => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_enum_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        let enum_type = AnalysedType::Enum(TypeEnum {
            cases: vec!["Variant1".to_string(), "Variant2".to_string(), "Variant3".to_string()],
        });

        let schema = converter.convert_type(&enum_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "string");
                assert_eq!(schema.enum_items.len(), 3);
                assert!(schema.enum_items.contains(&Value::String("Variant1".to_string())));
                assert!(schema.enum_items.contains(&Value::String("Variant2".to_string())));
                assert!(schema.enum_items.contains(&Value::String("Variant3".to_string())));
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
        }
    }

    #[test]
    fn test_complex_nested_type() {
        let mut converter = RibConverter::new_openapi();
        let mut registry = Registry::new();

        // Create a complex nested type with all RIB features
        let nested_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::Enum(TypeEnum {
                        cases: vec!["Active".to_string(), "Inactive".to_string()],
                    }),
                },
                NameTypePair {
                    name: "data".to_string(),
                    typ: AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "value".to_string(),
                                typ: AnalysedType::Variant(TypeVariant {
                                    cases: vec![
                                        NameOptionTypePair {
                                            name: "Number".to_string(),
                                            typ: Some(AnalysedType::U32(TypeU32)),
                                        },
                                        NameOptionTypePair {
                                            name: "Text".to_string(),
                                            typ: Some(AnalysedType::Str(TypeStr)),
                                        },
                                    ],
                                }),
                            },
                            NameTypePair {
                                name: "tags".to_string(),
                                typ: AnalysedType::List(TypeList {
                                    inner: Box::new(AnalysedType::Str(TypeStr)),
                                }),
                            },
                        ],
                    }),
                },
            ],
        });

        let schema = converter.convert_type(&nested_type, &mut registry).unwrap();
        match schema {
            MetaSchemaRef::Inline(schema) => {
                assert_eq!(schema.ty, "object");
                assert!(has_property(&schema.properties, "id"));
                assert!(has_property(&schema.properties, "status"));
                assert!(has_property(&schema.properties, "data"));
                assert_eq!(schema.required.len(), 3);
                assert!(schema.required.contains(&"id"));
                assert!(schema.required.contains(&"status"));
                assert!(schema.required.contains(&"data"));

                // Verify id field
                let id_schema = find_property(&schema.properties, "id").unwrap();
                assert_schema_type(id_schema, "integer");
                assert_schema_format(id_schema, "int32");

                // Verify status field (enum)
                let status_schema = find_property(&schema.properties, "status").unwrap();
                match status_schema {
                    MetaSchemaRef::Inline(schema) => {
                        assert_eq!(schema.ty, "string");
                        assert_eq!(schema.enum_items.len(), 2);
                        assert!(schema.enum_items.contains(&Value::String("Active".to_string())));
                        assert!(schema.enum_items.contains(&Value::String("Inactive".to_string())));
                    },
                    _ => panic!("Expected inline schema"),
                }

                // Verify data field (record)
                let data_schema = find_property(&schema.properties, "data").unwrap();
                match data_schema {
                    MetaSchemaRef::Inline(schema) => {
                        assert_eq!(schema.ty, "object");
                        assert!(has_property(&schema.properties, "value"));
                        assert!(has_property(&schema.properties, "tags"));

                        // Verify value field (variant)
                        let value_schema = find_property(&schema.properties, "value").unwrap();
                        match value_schema {
                            MetaSchemaRef::Inline(schema) => {
                                assert_eq!(schema.ty, "object");
                                assert!(has_property(&schema.properties, "type"));
                                assert!(schema.required.contains(&"type"));
                            },
                            _ => panic!("Expected inline schema"),
                        }

                        // Verify tags field (list)
                        let tags_schema = find_property(&schema.properties, "tags").unwrap();
                        match tags_schema {
                            MetaSchemaRef::Inline(schema) => {
                                assert_eq!(schema.ty, "array");
                                assert!(schema.items.is_some());
                            },
                            _ => panic!("Expected inline schema"),
                        }
                    },
                    _ => panic!("Expected inline schema"),
                }
            },
            MetaSchemaRef::Reference(_) => panic!("Expected inline schema"),
        }
    }
}