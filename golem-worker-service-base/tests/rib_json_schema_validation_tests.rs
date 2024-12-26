#[cfg(test)]
mod rib_json_schema_validation_tests {
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
    use valico::json_schema;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_ast::analysis::{
        AnalysedType,
        TypeBool,
        TypeStr,
        TypeU32,
        TypeVariant,
        TypeRecord,
        TypeList,
        TypeOption,
        NameOptionTypePair,
        NameTypePair,
    };
    use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
    use utoipa::openapi::Schema;
    use serde_json::Value;

    fn validate_json_against_schema(json: Value, schema: &Schema) -> bool {
        let schema_json = serde_json::to_value(schema).unwrap();
        let mut scope = json_schema::Scope::new();
        let schema = scope.compile_and_return(schema_json, false).unwrap();
        schema.validate(&json).is_valid()
    }

    fn create_rib_value(value: &str, typ: &AnalysedType) -> TypeAnnotatedValue {
        let json_value: Value = serde_json::from_str(value).unwrap();
        let parsed_value = TypeAnnotatedValue::parse_with_type(&json_value, typ)
            .unwrap();
        parsed_value
    }

    #[test]
    fn test_primitive_json_schema_validation() {
        let converter = RibConverter;

        // Test boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).unwrap();
        let rib_value = create_rib_value("true", &bool_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test integer
        let int_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&int_type).unwrap();
        let rib_value = create_rib_value("42", &int_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test string
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).unwrap();
        let rib_value = create_rib_value("\"hello\"", &str_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_record_json_schema_validation() {
        let converter = RibConverter;

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
            ],
        });

        let schema = converter.convert_type(&record_type).unwrap();
        let json_str = r#"{"field1": 42, "field2": "hello"}"#;
        let rib_value = create_rib_value(json_str, &record_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_variant_json_schema_validation() {
        let converter = RibConverter;

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
            ],
        });

        let schema = converter.convert_type(&variant_type).unwrap();
        
        // Test Case1
        let json_str = r#"{"discriminator": "Case1", "value": {"Case1": 42}}"#;
        let rib_value = create_rib_value(json_str, &variant_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test Case2
        let json_str = r#"{"discriminator": "Case2", "value": {"Case2": "hello"}}"#;
        let rib_value = create_rib_value(json_str, &variant_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_list_json_schema_validation() {
        let converter = RibConverter;

        let list_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::U32(TypeU32)),
        });

        let schema = converter.convert_type(&list_type).unwrap();
        let json_str = "[1, 2, 3, 4, 5]";
        let rib_value = create_rib_value(json_str, &list_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_complex_nested_json_schema_validation() {
        let converter = RibConverter;

        // Create a record containing a list of variants
        let variant_type = AnalysedType::Variant(TypeVariant {
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
        });

        let list_type = AnalysedType::List(TypeList {
            inner: Box::new(variant_type),
        });

        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "items".to_string(),
                    typ: list_type,
                },
                NameTypePair {
                    name: "name".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        });

        let schema = converter.convert_type(&record_type).unwrap();
        
        let json_str = r#"{
            "items": [
                {"discriminator": "Number", "value": {"Number": 42}},
                {"discriminator": "Text", "value": {"Text": "hello"}}
            ],
            "name": "test"
        }"#;

        let rib_value = create_rib_value(json_str, &record_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_invalid_json_schema_validation() {
        let converter = RibConverter;

        // Test with wrong type
        let int_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&int_type).unwrap();
        let json = serde_json::json!("not a number");
        assert!(!validate_json_against_schema(json, &schema));

        // Test with missing required field
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "required_field".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        });
        let schema = converter.convert_type(&record_type).unwrap();
        let json = serde_json::json!({});
        assert!(!validate_json_against_schema(json, &schema));

        // Test with wrong variant case
        let variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Case1".to_string(),
                    typ: Some(AnalysedType::U32(TypeU32)),
                },
            ],
        });
        let schema = converter.convert_type(&variant_type).unwrap();
        let json = serde_json::json!({
            "discriminator": "NonexistentCase",
            "value": {"NonexistentCase": 42}
        });
        assert!(!validate_json_against_schema(json, &schema));
    }

    #[test]
    fn test_negative_primitive_validation() {
        let converter = RibConverter;

        // Test wrong type for boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).unwrap();
        let invalid_json = serde_json::json!(42); // number instead of boolean
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test wrong type for integer
        let int_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&int_type).unwrap();
        let invalid_json = serde_json::json!("42"); // string instead of number
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test wrong type for string
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).unwrap();
        let invalid_json = serde_json::json!(true); // boolean instead of string
        assert!(!validate_json_against_schema(invalid_json, &schema));
    }

    #[test]
    fn test_negative_record_validation() {
        let converter = RibConverter;
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "required_field".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        });

        let schema = converter.convert_type(&record_type).unwrap();

        // Test missing required field
        let invalid_json = serde_json::json!({});
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test wrong type for field
        let invalid_json = serde_json::json!({
            "required_field": "not a number"
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test extra unknown field
        let invalid_json = serde_json::json!({
            "required_field": 42,
            "unknown_field": "extra"
        });
        // This should still validate as extra properties are allowed by default
        assert!(validate_json_against_schema(invalid_json, &schema));
    }

    #[test]
    fn test_negative_variant_validation() {
        let converter = RibConverter;
        let variant_type = AnalysedType::Variant(TypeVariant {
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
        });

        let schema = converter.convert_type(&variant_type).unwrap();

        // Test missing discriminator
        let invalid_json = serde_json::json!({
            "value": { "Number": 42 }
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test invalid discriminator
        let invalid_json = serde_json::json!({
            "discriminator": "InvalidVariant",
            "value": { "InvalidVariant": 42 }
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test mismatched value type
        let invalid_json = serde_json::json!({
            "discriminator": "Number",
            "value": { "Number": "not a number" }
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test mismatched variant name in value
        let invalid_json = serde_json::json!({
            "discriminator": "Number",
            "value": { "Text": 42 }
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));
    }

    #[test]
    fn test_negative_list_validation() {
        let converter = RibConverter;
        let list_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::U32(TypeU32)),
        });

        let schema = converter.convert_type(&list_type).unwrap();

        // Test non-array value
        let invalid_json = serde_json::json!(42);
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test array with wrong element types
        let invalid_json = serde_json::json!([1, "two", 3]);
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test empty array (should be valid)
        let valid_json = serde_json::json!([]);
        assert!(validate_json_against_schema(valid_json, &schema));
    }

    #[test]
    fn test_negative_option_validation() {
        let converter = RibConverter;
        let option_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::U32(TypeU32)),
        });

        let schema = converter.convert_type(&option_type).unwrap();

        // Test missing value field
        let invalid_json = serde_json::json!({});
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test wrong type for value
        let invalid_json = serde_json::json!({
            "value": "not a number"
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test null value (should be valid for Option)
        let valid_json = serde_json::json!({
            "value": null
        });
        assert!(validate_json_against_schema(valid_json, &schema));
    }

    #[test]
    fn test_negative_complex_nested_validation() {
        let converter = RibConverter;
        
        // Create a complex type: Record containing a list of variants
        let complex_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "items".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Variant(TypeVariant {
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
                        })),
                    }),
                },
            ],
        });

        let schema = converter.convert_type(&complex_type).unwrap();

        // Test invalid list element (missing discriminator)
        let invalid_json = serde_json::json!({
            "items": [
                { "value": { "Number": 42 } }
            ]
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test invalid variant value type
        let invalid_json = serde_json::json!({
            "items": [
                {
                    "discriminator": "Number",
                    "value": { "Number": "not a number" }
                }
            ]
        });
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test missing required field
        let invalid_json = serde_json::json!({});
        assert!(!validate_json_against_schema(invalid_json, &schema));

        // Test valid complex structure
        let valid_json = serde_json::json!({
            "items": [
                {
                    "discriminator": "Number",
                    "value": { "Number": 42 }
                },
                {
                    "discriminator": "Text",
                    "value": { "Text": "hello" }
                }
            ]
        });
        assert!(validate_json_against_schema(valid_json, &schema));
    }
} 