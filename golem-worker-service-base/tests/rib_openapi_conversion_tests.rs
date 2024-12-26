#[cfg(test)]
mod rib_openapi_conversion_tests {
    use golem_wasm_ast::analysis::{
        AnalysedType,
        TypeBool,
        TypeChr,
        TypeEnum,
        TypeF32,
        TypeF64,
        TypeList,
        TypeOption,
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
    use utoipa::openapi::{Schema, RefOr, schema::{ArrayItems, SchemaType}};
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::{RibConverter, CustomSchemaType};

    // Wrapper types for testing
    struct TestRibConverter(RibConverter);
    struct TestTypeStr(TypeStr);
    struct TestTypeList(TypeList);

    impl TestRibConverter {
        fn new() -> Self {
            TestRibConverter(RibConverter)
        }

        fn convert_type(&self, typ: &AnalysedType) -> Option<Schema> {
            self.0.convert_type(typ)
        }
    }

    impl TestTypeStr {
        fn new() -> Self {
            TestTypeStr(TypeStr)
        }
    }

    impl TestTypeList {
        fn new(inner: Box<AnalysedType>) -> Self {
            TestTypeList(TypeList { inner })
        }
    }

    // Helper function to verify schema type
    fn assert_schema_type(schema: &Schema, expected_type: CustomSchemaType) {
        match schema {
            Schema::Object(obj) => {
                let schema_type = match &obj.schema_type {
                    SchemaType::Type(t) => CustomSchemaType::from(t.clone()),
                    SchemaType::Array(_) => panic!("Expected single type, got array"),
                    SchemaType::AnyValue => panic!("Expected single type, got any value"),
                };
                assert_eq!(schema_type, expected_type);
            },
            Schema::Array(arr) => {
                match &arr.items {
                    ArrayItems::RefOrSchema(item) => {
                        match get_schema_from_ref_or(item) {
                            Schema::Object(obj) => {
                                let schema_type = match &obj.schema_type {
                                    SchemaType::Type(t) => CustomSchemaType::from(t.clone()),
                                    SchemaType::Array(_) => panic!("Expected single type, got array"),
                                    SchemaType::AnyValue => panic!("Expected single type, got any value"),
                                };
                                assert_eq!(schema_type, expected_type);
                            },
                            _ => panic!("Array items should be a Schema::Object"),
                        }
                    },
                    ArrayItems::False => panic!("Expected array items, got False"),
                }
            },
            _ => panic!("Unexpected schema type"),
        }
    }

    // Helper function to get schema from RefOr<Schema>
    fn get_schema_from_ref_or(schema_ref: &RefOr<Schema>) -> &Schema {
        match schema_ref {
            RefOr::T(schema) => schema,
            RefOr::Ref { .. } => panic!("Expected Schema, got Ref"),
        }
    }

    #[test]
    fn test_primitive_types() {
        let converter = TestRibConverter::new();

        // Boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Boolean);

        // Integer types
        let u8_type = AnalysedType::U8(TypeU8);
        let schema = converter.convert_type(&u8_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let u16_type = AnalysedType::U16(TypeU16);
        let schema = converter.convert_type(&u16_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let u32_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&u32_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let u64_type = AnalysedType::U64(TypeU64);
        let schema = converter.convert_type(&u64_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let s8_type = AnalysedType::S8(TypeS8);
        let schema = converter.convert_type(&s8_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let s16_type = AnalysedType::S16(TypeS16);
        let schema = converter.convert_type(&s16_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let s32_type = AnalysedType::S32(TypeS32);
        let schema = converter.convert_type(&s32_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        let s64_type = AnalysedType::S64(TypeS64);
        let schema = converter.convert_type(&s64_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Integer);

        // Float types
        let f32_type = AnalysedType::F32(TypeF32);
        let schema = converter.convert_type(&f32_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Number);

        let f64_type = AnalysedType::F64(TypeF64);
        let schema = converter.convert_type(&f64_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::Number);

        // String and Char
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::String);

        let char_type = AnalysedType::Chr(TypeChr);
        let schema = converter.convert_type(&char_type).unwrap();
        assert_schema_type(&schema, CustomSchemaType::String);
    }

    #[test]
    fn test_list_type() {
        let converter = TestRibConverter::new();
        let inner_type = TestTypeStr::new();
        let list_type = TestTypeList::new(Box::new(AnalysedType::Str(inner_type.0)));
        let schema = converter.convert_type(&AnalysedType::List(list_type.0)).unwrap();

        if let Schema::Array(arr) = schema {
            match &arr.items {
                ArrayItems::RefOrSchema(item) => {
                    match get_schema_from_ref_or(item) {
                        Schema::Object(obj) => {
                            let schema_type = match &obj.schema_type {
                                SchemaType::Type(t) => CustomSchemaType::from(t.clone()),
                                SchemaType::Array(_) => panic!("Expected single type, got array"),
                                SchemaType::AnyValue => panic!("Expected single type, got any value"),
                            };
                            assert_eq!(schema_type, CustomSchemaType::String);
                        },
                        _ => panic!("Array items should be a Schema::Object"),
                    }
                },
                ArrayItems::False => panic!("Expected array items, got False"),
            }
        } else {
            panic!("Expected Schema::Array");
        }
    }

    #[test]
    fn test_record_type() {
        let converter = TestRibConverter::new();
        let field1_type = AnalysedType::U32(TypeU32);
        let field2_type = AnalysedType::Str(TypeStr);
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "field1".to_string(),
                    typ: *Box::new(field1_type),
                },
                NameTypePair {
                    name: "field2".to_string(),
                    typ: *Box::new(field2_type),
                },
            ],
        });

        let schema = converter.convert_type(&record_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert_schema_type(&schema, CustomSchemaType::Object);
                assert_eq!(obj.required.len(), 2);
                assert!(obj.required.contains(&"field1".to_string()));
                assert!(obj.required.contains(&"field2".to_string()));

                let field1_schema = get_schema_from_ref_or(obj.properties.get("field1").unwrap());
                assert_schema_type(field1_schema, CustomSchemaType::Integer);

                let field2_schema = get_schema_from_ref_or(obj.properties.get("field2").unwrap());
                assert_schema_type(field2_schema, CustomSchemaType::String);
            },
            _ => panic!("Expected object schema"),
        }
    }

    #[test]
    fn test_enum_type() {
        let converter = TestRibConverter::new();
        let enum_type = AnalysedType::Enum(TypeEnum {
            cases: vec!["Variant1".to_string(), "Variant2".to_string()],
        });

        let schema = converter.convert_type(&enum_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert_schema_type(&schema, CustomSchemaType::String);
                let enum_values = obj.enum_values.as_ref().unwrap();
                assert_eq!(enum_values.len(), 2);
                assert!(enum_values.contains(&Value::String("Variant1".to_string())));
                assert!(enum_values.contains(&Value::String("Variant2".to_string())));
            },
            _ => panic!("Expected object schema"),
        }
    }

    #[test]
    fn test_variant_type() {
        let converter = TestRibConverter::new();
        let variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Variant1".to_string(),
                    typ: Some(*Box::new(AnalysedType::U32(TypeU32))),
                },
                NameOptionTypePair {
                    name: "Variant2".to_string(),
                    typ: None,
                },
            ],
        });

        let schema = converter.convert_type(&variant_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert_schema_type(&schema, CustomSchemaType::Object);
                assert!(obj.properties.contains_key("discriminator"));
                assert!(obj.properties.contains_key("value"));

                let discriminator = get_schema_from_ref_or(obj.properties.get("discriminator").unwrap());
                assert_schema_type(discriminator, CustomSchemaType::String);

                let value = get_schema_from_ref_or(obj.properties.get("value").unwrap());
                if let Schema::OneOf(one_of) = value {
                    // Verify variant schemas
                    assert_eq!(one_of.items.len(), 2);
                    // Additional variant schema verification could be added here
                } else {
                    panic!("Expected OneOf schema for value");
                }
            },
            _ => panic!("Expected object schema"),
        }
    }

    #[test]
    fn test_option_type() {
        let converter = TestRibConverter::new();
        let option_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::U32(TypeU32)),
        });

        let schema = converter.convert_type(&option_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert_schema_type(&schema, CustomSchemaType::Object);
                assert!(obj.properties.contains_key("value"));
                assert!(obj.required.is_empty()); // Optional field

                let value_schema = get_schema_from_ref_or(obj.properties.get("value").unwrap());
                assert_schema_type(value_schema, CustomSchemaType::Integer);
            },
            _ => panic!("Expected object schema"),
        }
    }

    #[test]
    fn test_result_type() {
        let converter = TestRibConverter::new();
        let result_type = AnalysedType::Result(TypeResult {
            ok: Some(Box::new(AnalysedType::U32(TypeU32))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        });

        let schema = converter.convert_type(&result_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert_schema_type(&schema, CustomSchemaType::Object);
                assert!(obj.properties.contains_key("ok"));
                assert!(obj.properties.contains_key("err"));

                let ok_schema = get_schema_from_ref_or(obj.properties.get("ok").unwrap());
                assert_schema_type(ok_schema, CustomSchemaType::Integer);

                let err_schema = get_schema_from_ref_or(obj.properties.get("err").unwrap());
                assert_schema_type(err_schema, CustomSchemaType::String);
            },
            _ => panic!("Expected object schema"),
        }
    }

    #[test]
    fn test_complex_nested_type() {
        let converter = TestRibConverter::new();
        let inner_type = TestTypeStr::new();
        let list_type = TestTypeList::new(Box::new(AnalysedType::Str(inner_type.0)));
        let schema = converter.convert_type(&AnalysedType::List(list_type.0)).unwrap();

        if let Schema::Array(arr) = schema {
            match &arr.items {
                ArrayItems::RefOrSchema(item) => {
                    match get_schema_from_ref_or(item) {
                        Schema::Object(obj) => {
                            let schema_type = match &obj.schema_type {
                                SchemaType::Type(t) => CustomSchemaType::from(t.clone()),
                                SchemaType::Array(_) => panic!("Expected single type, got array"),
                                SchemaType::AnyValue => panic!("Expected single type, got any value"),
                            };
                            assert_eq!(schema_type, CustomSchemaType::String);
                        },
                        _ => panic!("Array items should be a Schema::Object"),
                    }
                },
                ArrayItems::False => panic!("Expected array items, got False"),
            }
        } else {
            panic!("Expected Schema::Array");
        }
    }

    #[test]
    fn test_convert_input_type() {
        use rib::RibInputTypeInfo;
        use std::collections::HashMap;

        let converter = TestRibConverter::new();

        // Test empty input type
        let empty_input = RibInputTypeInfo {
            types: HashMap::new(),
        };
        assert!(converter.0.convert_input_type(&empty_input).is_none());

        // Test input type with single field
        let mut single_field_input = RibInputTypeInfo {
            types: HashMap::new(),
        };
        single_field_input.types.insert(
            "field1".to_string(),
            AnalysedType::U32(TypeU32),
        );
        let schema = converter.0.convert_input_type(&single_field_input).unwrap();
        match schema {
            Schema::Object(obj) => {
                assert_eq!(obj.properties.len(), 1);
                assert!(obj.properties.contains_key("field1"));
                let field_schema = get_schema_from_ref_or(obj.properties.get("field1").unwrap());
                assert_schema_type(field_schema, CustomSchemaType::Integer);
            },
            _ => panic!("Expected object schema"),
        }

        // Test input type with multiple fields of different types
        let mut multi_field_input = RibInputTypeInfo {
            types: HashMap::new(),
        };
        multi_field_input.types.insert(
            "string_field".to_string(),
            AnalysedType::Str(TypeStr),
        );
        multi_field_input.types.insert(
            "bool_field".to_string(),
            AnalysedType::Bool(TypeBool),
        );
        multi_field_input.types.insert(
            "number_field".to_string(),
            AnalysedType::F64(TypeF64),
        );
        let schema = converter.0.convert_input_type(&multi_field_input).unwrap();
        match schema {
            Schema::Object(obj) => {
                assert_eq!(obj.properties.len(), 3);
                
                // Check string field
                assert!(obj.properties.contains_key("string_field"));
                let string_schema = get_schema_from_ref_or(obj.properties.get("string_field").unwrap());
                assert_schema_type(string_schema, CustomSchemaType::String);
                
                // Check bool field
                assert!(obj.properties.contains_key("bool_field"));
                let bool_schema = get_schema_from_ref_or(obj.properties.get("bool_field").unwrap());
                assert_schema_type(bool_schema, CustomSchemaType::Boolean);
                
                // Check number field
                assert!(obj.properties.contains_key("number_field"));
                let number_schema = get_schema_from_ref_or(obj.properties.get("number_field").unwrap());
                assert_schema_type(number_schema, CustomSchemaType::Number);
            },
            _ => panic!("Expected object schema"),
        }

        // Test input type with complex nested types
        let mut complex_input = RibInputTypeInfo {
            types: HashMap::new(),
        };
        
        // Add a list type
        complex_input.types.insert(
            "list_field".to_string(),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::U32(TypeU32)),
            }),
        );
        
        // Add a record type
        complex_input.types.insert(
            "record_field".to_string(),
            AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "sub_field".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            }),
        );
        
        let schema = converter.0.convert_input_type(&complex_input).unwrap();
        match schema {
            Schema::Object(obj) => {
                assert_eq!(obj.properties.len(), 2);
                
                // Check list field
                assert!(obj.properties.contains_key("list_field"));
                let list_schema = get_schema_from_ref_or(obj.properties.get("list_field").unwrap());
                match list_schema {
                    Schema::Array(_) => (),
                    _ => panic!("Expected array schema for list field"),
                }
                
                // Check record field
                assert!(obj.properties.contains_key("record_field"));
                let record_schema = get_schema_from_ref_or(obj.properties.get("record_field").unwrap());
                match record_schema {
                    Schema::Object(record_obj) => {
                        assert!(record_obj.properties.contains_key("sub_field"));
                        let sub_field_schema = get_schema_from_ref_or(record_obj.properties.get("sub_field").unwrap());
                        assert_schema_type(sub_field_schema, CustomSchemaType::String);
                    },
                    _ => panic!("Expected object schema for record field"),
                }
            },
            _ => panic!("Expected object schema"),
        }
    }
} 